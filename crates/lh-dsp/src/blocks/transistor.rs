//! Transistor stages solved **nodally** — the white-box circuits that are not
//! wave digital filters (Tone Revolution phase 05).
//!
//! # Why these are not in [`crate::blocks::wdf`]
//!
//! WDF is the right substrate when a circuit reduces to a tree of linear
//! one-ports with a *single* nonlinearity at the root: the tree hands the root
//! one incident wave and one resistance, and the root solves a scalar equation
//! on its own v–i curve. Every op-amp overdrive in the drive family fits that
//! shape, which is why phase 03/04 could build them all out of adaptors.
//!
//! Transistor circuits do not, for two different reasons, and this module holds
//! one answer to each:
//!
//! - [`ShuntFeedbackStage`] — a fixed-gain inverting amplifier with the
//!   clipping diodes **inside** its feedback network (the Big Muff's clipping
//!   stage). This *could* be a WDF tree, but the amplifier here is a
//!   common-emitter stage linearised to a bare voltage gain `A = −Rc/Re`: it has
//!   no meaningful input or output impedance to give an R-type junction, so the
//!   honest model is the node equation itself, solved by Newton.
//! - [`Bjt`] — the Ebers–Moll junction pair. A bipolar transistor is a
//!   **two-port** nonlinearity (two coupled exponentials sharing the base), so
//!   it cannot be a WDF root at all: it needs a real multi-node solve, which
//!   [`Bjt::stamp`] provides in the linearised (companion) form a nodal Newton
//!   iteration consumes.
//!
//! Both obey the same conventions the WDF roots established: `f64` inside,
//! warm-started from last sample, damped so a slammed cold start cannot
//! overshoot into the exponential, and a fixed iteration ceiling so the cost is
//! bounded on the audio thread (RT rules 1 and 7).
//!
//! # Provenance
//!
//! Circuit topologies, component values and device parameters are facts read
//! off schematics and datasheets. The equations are the textbook Ebers–Moll
//! transport model and ordinary nodal analysis; the discretisation is the
//! bilinear (trapezoidal) companion model. No GPL sources were copied.

use super::wdf::flush;

/// Newton iteration ceiling, shared by both solvers. Warm-started audio needs
/// 1–3; a cold, slammed start converges in well under this because the steps
/// are damped.
const MAX_ITERS: usize = 24;
/// Convergence tolerance on the solved voltage, in volts.
const TOL: f64 = 1e-10;
/// Exponent clamp — pure overflow paranoia for a pathological caller. `e^60`
/// is 1e26, which stays finite in `f64` however it is scaled.
const EXP_CLAMP: f64 = 60.0;
/// …and the reverse end, past which a junction is simply *off* (see
/// [`Bjt::stamp`]).
const EXP_OFF: f64 = 40.0;

// --- the Big Muff's clipping stage -------------------------------------------

/// An inverting amplifier of fixed voltage gain `A`, driven through a Thévenin
/// source resistance, with a **nonlinear feedback network** from its output
/// back to its summing node: a resistor, a capacitor across it, and an
/// antiparallel diode pair across both.
///
/// This is the Big Muff Pi's clipping stage ([`crate::drive`]'s `big-muff`),
/// and it is the *same mechanism class* as `sd1`'s feedback overdrive — the
/// diodes clip the feedback impedance, not the signal — with the op-amp
/// replaced by a common-emitter transistor stage linearised to `A = −Rc/Re`.
///
/// # The equation
///
/// Let `y` be the output and `u` the Thévenin (open-circuit) voltage at the
/// summing node. The amplifier fixes the summing node at `y/A` exactly, so the
/// voltage across the feedback network is
///
/// ```text
/// v = y − y/A = κ·y,        κ = 1 − 1/A
/// ```
///
/// and the current that network pushes back into the summing node is
///
/// ```text
/// i(v) = 2·Is·sinh(v / nVt)  +  v·G_f  +  (v·G_c − s_c)
/// ```
///
/// — diodes, feedback resistor, and the capacitor's bilinear companion. KCL at
/// the summing node is then one scalar equation in `y`:
///
/// ```text
/// F(y) = y/A − u − R_th·i(κ·y) = 0
/// ```
///
/// `A < 0` and `di/dv > 0`, so `F′ < 0` everywhere: the root is unique and
/// Newton cannot land on a stationary point.
///
/// # Why `R_th` is the *AC* Thévenin resistance
///
/// The summing node of a Big Muff stage sees the source through `C5 + R19` and
/// a bias resistor `R20` to AC ground. Its open-circuit voltage is the familiar
/// high-pass `s·C5·R20 / (1 + s·C5·(R19+R20))` — but the resistance the
/// feedback current is injected across is `R19 ‖ R20`, not `R20`. The two are
/// an order of magnitude apart (9.1 kΩ vs 100 kΩ on stock values) and the
/// closed-loop gain is `≈ −1/(1/|A| + R_th/R_f)`, so getting it wrong moves the
/// stage's small-signal gain by ~6×. `drive::big_muff` pins this against a
/// hand-solved AC analysis.
pub struct ShuntFeedbackStage {
    /// Open-loop voltage gain `A`, negative (inverting).
    gain: f64,
    /// `1 − 1/A` — the feedback network sees the output *minus* the summing
    /// node, and the summing node is exactly `y/A`.
    kappa: f64,
    /// Thévenin resistance the feedback current is injected across.
    r_th: f64,
    /// Feedback resistor conductance.
    g_f: f64,
    /// Feedback capacitance, farads — kept so [`prepare`](Self::prepare) can
    /// re-derive its bilinear conductance at a new rate.
    c_f: f64,
    /// `2·C_f·fs`, the capacitor's bilinear conductance.
    g_c: f64,
    /// `2·Is` for the antiparallel pair, and its thermal scale `n·Vt`.
    two_is: f64,
    vt_n: f64,
    /// Newton step cap, in volts of `y`: ten thermal scales' worth of the
    /// feedback voltage.
    dy_max: f64,
    /// Bilinear history current of the feedback capacitor.
    s_c: f64,
    /// Warm start: the output solved last sample.
    y: f64,
}

impl ShuntFeedbackStage {
    /// `gain` is the amplifier's open-loop gain (negative); `r_th` the source
    /// resistance at the summing node; `r_f`/`c_f` the feedback network; and
    /// `is`/`n`/`vt` the diode pair's junction parameters (per ADR 033 a device
    /// carries both `Is` *and* its ideality `n`).
    pub fn new(gain: f32, r_th: f32, r_f: f32, c_f: f32, is: f32, n: f32, vt: f32) -> Self {
        debug_assert!(gain < 0.0, "an inverting stage has A < 0");
        let gain = f64::from(gain);
        let vt_n = f64::from(n) * f64::from(vt);
        let kappa = 1.0 - 1.0 / gain;
        Self {
            gain,
            kappa,
            r_th: f64::from(r_th),
            g_f: 1.0 / f64::from(r_f),
            c_f: f64::from(c_f),
            g_c: 2.0 * f64::from(c_f) * 48_000.0,
            two_is: 2.0 * f64::from(is),
            vt_n,
            dy_max: 10.0 * vt_n / kappa,
            s_c: 0.0,
            y: 0.0,
        }
    }

    /// Re-derive the capacitor's bilinear conductance at `sample_rate` (the
    /// *oversampled* rate, since that is where the solver runs).
    pub fn prepare(&mut self, sample_rate: f32) {
        self.g_c = 2.0 * self.c_f * f64::from(sample_rate);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.s_c = 0.0;
        self.y = 0.0;
    }

    /// Swap the clipping device — `is`/`n`/`vt` are one diode's parameters.
    pub fn set_diode(&mut self, is: f32, n: f32, vt: f32) {
        debug_assert!(is > 0.0 && n > 0.0 && vt > 0.0);
        self.two_is = 2.0 * f64::from(is);
        self.vt_n = f64::from(n) * f64::from(vt);
        self.dy_max = 10.0 * self.vt_n / self.kappa;
    }

    /// The stage's small-signal gain about zero, with the capacitor open.
    ///
    /// The diodes are *not* simply "off": an antiparallel pair has a finite
    /// zero-bias resistance `nVt / 2Is` — 9 MΩ for a 1N4148 pair — which sits
    /// across the feedback resistor and, at the Big Muff's 470 kΩ, takes about
    /// 3 % off the gain. Leaving it out is the classic way for a hand analysis
    /// to disagree with a working model by a few percent.
    pub fn dc_gain(&self) -> f64 {
        let g_d = self.two_is / self.vt_n;
        1.0 / (1.0 / self.gain - self.r_th * self.kappa * (self.g_f + g_d))
    }

    /// One sample. `u` is the Thévenin voltage at the summing node; the return
    /// is the stage output `y`.
    ///
    /// **RT-safe:** no allocation, bounded iteration, `f64` throughout, and the
    /// step is damped so a cold slam cannot stall in the exponential.
    #[inline]
    pub fn process(&mut self, u: f32) -> f32 {
        let u = f64::from(u);
        let mut y = self.y;
        for _ in 0..MAX_ITERS {
            let v = self.kappa * y;
            let x = (v / self.vt_n).clamp(-EXP_CLAMP, EXP_CLAMP);
            let e = x.exp();
            let einv = 1.0 / e;
            // sinh and cosh from one exp — the pair's current and its slope.
            let i = self.two_is * 0.5 * (e - einv) + v * (self.g_f + self.g_c) - self.s_c;
            let gi = self.two_is * 0.5 * (e + einv) / self.vt_n + self.g_f + self.g_c;
            let f = y / self.gain - u - self.r_th * i;
            let fp = 1.0 / self.gain - self.r_th * self.kappa * gi;
            let dy = (f / fp).clamp(-self.dy_max, self.dy_max);
            y -= dy;
            if dy.abs() < TOL {
                break;
            }
        }
        self.y = y;
        // Advance the capacitor at the solved node voltage.
        self.s_c = 2.0 * self.g_c * (self.kappa * y) - self.s_c;
        flush(y as f32)
    }

    /// The residual of the node equation at the last solved point, in volts.
    ///
    /// `s_c` must be the capacitor state the solve *ran with* — [`process`] has
    /// already advanced it by the time this is callable, and using the advanced
    /// one measures a different equation.
    #[cfg(test)]
    fn residual(&self, u: f32, s_c: f64) -> f64 {
        let y = self.y;
        let v = self.kappa * y;
        let i = self.two_is * (v / self.vt_n).sinh() + v * (self.g_f + self.g_c) - s_c;
        y / self.gain - f64::from(u) - self.r_th * i
    }
}

// --- the Ebers–Moll bipolar junction transistor ------------------------------

/// Which way the junctions point. The device's equations are written for an
/// NPN; a PNP is the same device with every terminal voltage and current
/// negated, which is exactly what [`Bjt::stamp`] does with this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Polarity {
    Npn,
    Pnp,
}

impl Polarity {
    #[inline]
    fn sign(self) -> f64 {
        match self {
            Polarity::Npn => 1.0,
            Polarity::Pnp => -1.0,
        }
    }
}

/// The linearised (companion) form of the transistor at one operating point:
/// the three terminal currents and the four transconductances a nodal Newton
/// iteration needs to build its Jacobian.
///
/// Currents follow the **external** convention: `ib` and `ic` flow *into* the
/// base and collector terminals, `ie` flows *out of* the emitter — so
/// `ib + ic = ie` holds identically, whatever the polarity.
#[derive(Clone, Copy, Default, Debug)]
pub struct Stamp {
    pub ib: f64,
    pub ic: f64,
    pub ie: f64,
    /// `∂ib/∂v_be`, `∂ib/∂v_bc` — and likewise for the other two, in the
    /// device's own (NPN) variables. They are polarity-independent: flipping
    /// both the voltage and the current leaves a conductance alone.
    pub gib_be: f64,
    pub gib_bc: f64,
    pub gic_be: f64,
    pub gic_bc: f64,
    pub gie_be: f64,
    pub gie_bc: f64,
}

/// A bipolar junction transistor in the **Ebers–Moll transport form** —
/// two exponential junctions sharing a base, coupled by the forward and
/// reverse current gains:
///
/// ```text
/// i_f = Is·(exp(v_be/Vt) − 1)          forward (base–emitter) junction
/// i_r = Is·(exp(v_bc/Vt) − 1)          reverse (base–collector) junction
///
/// ic  = i_f − i_r·(1 + 1/βR)
/// ie  = i_f·(1 + 1/βF) − i_r
/// ib  = i_f/βF + i_r/βR
/// ```
///
/// Both of the transistor's clipping mechanisms fall straight out of this and
/// neither needs a curve: **saturation** is the reverse junction turning on as
/// the collector swings back toward the base, and **cutoff** is the forward
/// junction turning off. They are not symmetric, which is why a one-transistor
/// booster grows even harmonics where a diode pair does not.
///
/// This is a *two-port* nonlinearity, so unlike a diode it can never be a WDF
/// root: [`stamp`](Self::stamp) hands its caller the linearisation, and the
/// caller runs the multi-node Newton (see `drive::rangemaster`).
pub struct Bjt {
    is: f64,
    vt: f64,
    beta_f: f64,
    beta_r: f64,
    polarity: Polarity,
}

impl Bjt {
    /// `is` = junction saturation current, `vt` = thermal voltage, `beta_f` /
    /// `beta_r` = forward / reverse current gain.
    ///
    /// Germanium is not silicon with different numbers in the same decade: an
    /// OC44's `Is` is ~1e-7 A against a 2N3904's ~1e-14, which is the whole
    /// reason a germanium stage sits at a 0.2 V `Vbe` and conducts softly.
    pub fn new(is: f32, vt: f32, beta_f: f32, beta_r: f32, polarity: Polarity) -> Self {
        debug_assert!(is > 0.0 && vt > 0.0 && beta_f > 0.0 && beta_r > 0.0);
        Self {
            is: f64::from(is),
            vt: f64::from(vt),
            beta_f: f64::from(beta_f),
            beta_r: f64::from(beta_r),
            polarity,
        }
    }

    /// Thermal voltage, in volts — the caller's Newton damps its steps in
    /// multiples of it.
    pub fn vt(&self) -> f64 {
        self.vt
    }

    pub fn polarity(&self) -> Polarity {
        self.polarity
    }

    /// Linearise at the terminal voltages `(vb, ve, vc)`, measured against the
    /// same reference the caller's node equations use.
    #[inline]
    pub fn stamp(&self, vb: f64, ve: f64, vc: f64) -> Stamp {
        let p = self.polarity.sign();
        // Into the device's own variables: for a PNP these are v_eb and v_cb.
        let v_be = (p * (vb - ve) / self.vt).clamp(-EXP_CLAMP, EXP_CLAMP);
        let v_bc = (p * (vb - vc) / self.vt).clamp(-EXP_CLAMP, EXP_CLAMP);
        // A junction forty thermal voltages into reverse carries `−Is` and
        // nothing else: `e^−40` is 4e-18, and the conductance it contributes is
        // 1e-23 S against a Jacobian whose smallest entry is a microsiemens.
        // Zeroing it is exact to `f64` *and* skips a transcendental — which
        // matters, because in the forward-active region the collector junction
        // is always this far off, and that is where audio spends its time.
        let ef = if v_be < -EXP_OFF { 0.0 } else { v_be.exp() };
        let er = if v_bc < -EXP_OFF { 0.0 } else { v_bc.exp() };
        let i_f = self.is * (ef - 1.0);
        let i_r = self.is * (er - 1.0);
        // dI/dV of each junction.
        let gf = self.is * ef / self.vt;
        let gr = self.is * er / self.vt;
        let (rf, rr) = (1.0 / self.beta_f, 1.0 / self.beta_r);

        Stamp {
            // Back out to the external convention.
            ib: p * (i_f * rf + i_r * rr),
            ic: p * (i_f - i_r * (1.0 + rr)),
            ie: p * (i_f * (1.0 + rf) - i_r),
            gib_be: gf * rf,
            gib_bc: gr * rr,
            gic_be: gf,
            gic_bc: -gr * (1.0 + rr),
            gie_be: gf * (1.0 + rf),
            gie_bc: -gr,
        }
    }
}

/// Solve `J·x = b` in place for a 3×3 system by Gaussian elimination with
/// partial pivoting. Returns `None` if the matrix is numerically singular —
/// the caller then keeps its previous iterate rather than producing a `NaN`.
///
/// Three unknowns is small enough that the loop unrolls and no allocation is
/// involved, which is the point: this runs on the audio thread.
#[inline]
pub fn solve3(mut m: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for k in 0..3 {
        // Partial pivot: the node conductances here span ten decades (a 47 µF
        // bypass companion against a 470 kΩ bias leg), so this is not optional.
        let mut piv = k;
        for r in (k + 1)..3 {
            if m[r][k].abs() > m[piv][k].abs() {
                piv = r;
            }
        }
        if m[piv][k].abs() < 1e-300 {
            return None;
        }
        m.swap(k, piv);
        b.swap(k, piv);
        // Disjoint borrows of the pivot row and the rows below it.
        let (m_top, m_rest) = m.split_at_mut(k + 1);
        let (b_top, b_rest) = b.split_at_mut(k + 1);
        let (pivot, b_pivot) = (&m_top[k], b_top[k]);
        for (row, bv) in m_rest.iter_mut().zip(b_rest.iter_mut()) {
            let f = row[k] / pivot[k];
            if f != 0.0 {
                for (rc, pc) in row.iter_mut().zip(pivot.iter()).skip(k) {
                    *rc -= f * pc;
                }
                *bv -= f * b_pivot;
            }
        }
    }
    let mut x = [0.0f64; 3];
    for k in (0..3).rev() {
        let mut s = b[k];
        for c in (k + 1)..3 {
            s -= m[k][c] * x[c];
        }
        x[k] = s / m[k][k];
    }
    if x.iter().all(|v| v.is_finite()) {
        Some(x)
    } else {
        None
    }
}

/// Iteration ceiling and step cap for a nodal Newton over [`Bjt`]. Exposed so
/// a caller's solver reads the same constants the module's own tests do.
pub const NODAL_MAX_ITERS: usize = MAX_ITERS;
/// Junction-voltage damping, in thermal voltages: a Newton step that would move
/// either junction more than this is scaled back whole, keeping the direction
/// but not the length. Without it a cold start into `exp(x/Vt)` overshoots to
/// `e^400` and stalls.
pub const NODAL_DV_MAX_VT: f64 = 10.0;
/// Convergence tolerance on the node voltages, in volts.
///
/// Looser than the scalar [`TOL`] on purpose. Newton is quadratic here, so the
/// step size falls roughly 1e-3 → 1e-9 → 1e-18: a 1e-10 threshold buys a whole
/// extra iteration (25 % of the pedal's cost) to move the answer by a
/// picovolt. 1e-8 V is still forty times finer than `f32` can represent at the
/// couple of volts these nodes sit at, which is the real floor — the same
/// argument PRD 029 used to bound its oracle.
pub const NODAL_TOL: f64 = 1e-8;

#[cfg(test)]
mod tests {
    use super::*;

    // Stock Big Muff clipping stage: A = −Rc/Re, R19 ‖ R20 at the summing
    // node, 470 kΩ ‖ 470 pF ‖ 1N4148 pair in the feedback path.
    const A: f32 = -10_000.0 / 150.0;
    const R_TH: f32 = 9_090.909;
    const R_F: f32 = 470_000.0;
    const C_F: f32 = 470e-12;
    const IS: f32 = 2.52e-9;
    const N: f32 = 1.75;
    const VT: f32 = 25.85e-3;
    const OS: f32 = 4.0 * 48_000.0;

    fn stage() -> ShuntFeedbackStage {
        let mut s = ShuntFeedbackStage::new(A, R_TH, R_F, C_F, IS, N, VT);
        s.prepare(OS);
        s
    }

    /// The solver really solves its equation — the phase's first acceptance
    /// criterion. Checked over the whole range from "far below the knee" to
    /// "slammed", because the Jacobian's condition changes by ten decades
    /// across it.
    #[test]
    fn the_stage_solves_its_node_equation() {
        for &u in &[-1e3, -1.0, -0.05, -1e-4, 0.0, 1e-4, 0.05, 1.0, 1e3] {
            let mut s = stage();
            let before = s.s_c;
            let y = s.process(u);
            assert!(y.is_finite(), "u={u}: {y}");
            let r = s.residual(u, before);
            assert!(r.abs() < 1e-9, "u={u}: residual {r:e} V (y={y})");
        }
    }

    /// Below the knee the diodes are open and the stage is a plain inverting
    /// amplifier of finite gain. Its closed-loop gain must match the textbook
    /// figure for the same three numbers — a check that shares no arithmetic
    /// with [`ShuntFeedbackStage::process`].
    ///
    /// Settled, not instantaneous: `C_f` is a short at the first sample (its
    /// bilinear conductance is 180 µS against the feedback resistor's 2.1 µS),
    /// so a one-shot measurement reads the capacitor, not the resistor. The
    /// network's time constant is `R_f·C_f` = 221 µs — 42 samples at 192 kHz.
    #[test]
    fn the_small_signal_gain_matches_the_textbook_inverting_amplifier() {
        let mut s = stage();
        // The feedback impedance about zero is the resistor in parallel with
        // the diode pair's zero-bias resistance nVt/2Is.
        let r_d = f64::from(N * VT) / (2.0 * f64::from(IS));
        let r_f = 1.0 / (1.0 / f64::from(R_F) + 1.0 / r_d);
        // −(R_f/R_th) reduced by the finite open-loop gain: the standard
        // 1/(1 + (1 + R_f/R_th)/|A|) correction.
        let ideal = -r_f / f64::from(R_TH);
        let expected = ideal / (1.0 + (1.0 - ideal) / f64::from(-A));
        // Well below the diode knee, held until the capacitor has charged.
        let u = 1e-4f32;
        let mut y = 0.0;
        for _ in 0..4_000 {
            y = s.process(u);
        }
        let g = f64::from(y) / f64::from(u);
        assert!(
            (g / expected - 1.0).abs() < 0.02,
            "closed-loop gain {g:.3} vs textbook {expected:.3}"
        );
        // And the closed form the stage reports for its own linear region.
        assert!(
            (s.dc_gain() / expected - 1.0).abs() < 0.02,
            "{}",
            s.dc_gain()
        );
    }

    /// The knee is where the circuit says it is: the feedback diodes clamp the
    /// output near their forward drop no matter how hard the stage is driven —
    /// and, being an inverting stage, on the other side of zero.
    #[test]
    fn the_stage_clamps_at_the_diode_knee() {
        let mut s = stage();
        let mut y = 0.0;
        for _ in 0..64 {
            y = s.process(10.0);
        }
        assert!(y < 0.0, "an inverting stage inverts: {y}");
        assert!((0.3..0.9).contains(&y.abs()), "clamped output {y}");
    }

    /// RT rule 7 at the solver: a slammed, alternating input stays finite and
    /// bounded, cold, with no warm start to help it.
    #[test]
    fn the_stage_is_bounded_when_slammed() {
        let mut s = stage();
        for k in 0..1000 {
            let u = if k % 2 == 0 { 1.0e6 } else { -1.0e6 };
            let y = s.process(u);
            assert!(y.is_finite() && y.abs() < 2.0, "k={k}: y={y}");
        }
    }

    /// Silence in, exact silence out: `y = 0` is the fixed point of the node
    /// equation and every state stays at zero. (This is only true because the
    /// stage carries no bias offset — see PRD 032 §2.3.)
    #[test]
    fn the_stage_is_silent_on_silence() {
        let mut s = stage();
        for _ in 0..500 {
            assert_eq!(s.process(0.0), 0.0);
        }
    }

    // --- the transistor ---

    fn ge_pnp() -> Bjt {
        Bjt::new(1.0e-7, 25.85e-3, 100.0, 2.0, Polarity::Pnp)
    }

    /// Kirchhoff inside the device: whatever goes into the base and collector
    /// comes out of the emitter, at every operating point and both polarities.
    #[test]
    fn the_transistor_conserves_current() {
        for polarity in [Polarity::Npn, Polarity::Pnp] {
            let q = Bjt::new(1.0e-7, 25.85e-3, 100.0, 2.0, polarity);
            let p = polarity.sign();
            for &vbe in &[-0.3, 0.0, 0.1, 0.2, 0.3] {
                for &vbc in &[-5.0, -0.5, 0.0, 0.15] {
                    // Build terminal voltages that produce these junctions.
                    let (vb, ve, vc) = (0.0, -p * vbe, -p * vbc);
                    let s = q.stamp(vb, ve, vc);
                    let err = s.ib + s.ic - s.ie;
                    let scale = s.ie.abs().max(1e-12);
                    assert!(
                        err.abs() / scale < 1e-9,
                        "{polarity:?} {vbe}/{vbc}: {err:e}"
                    );
                }
            }
        }
    }

    /// The stamped conductances are the analytic derivatives of the stamped
    /// currents — the Jacobian a nodal Newton is about to trust. Checked by
    /// central difference in the device's own variables.
    #[test]
    fn the_stamp_derivatives_match_finite_differences() {
        let q = ge_pnp();
        let p = Polarity::Pnp.sign();
        let h = 1e-6;
        for &vbe in &[-0.1, 0.05, 0.15, 0.22] {
            for &vbc in &[-4.0, -0.2, 0.1] {
                let at = |dbe: f64, dbc: f64| q.stamp(0.0, -p * (vbe + dbe), -p * (vbc + dbc));
                let s = at(0.0, 0.0);
                // ∂/∂v_be, then ∂/∂v_bc, for each of the three currents. The
                // stamped currents carry the polarity sign, the conductances
                // do not, so compare against p·d(i)/d(v).
                let d_be = at(h, 0.0);
                let d_be_m = at(-h, 0.0);
                let d_bc = at(0.0, h);
                let d_bc_m = at(0.0, -h);
                for (num, ana, what) in [
                    ((d_be.ib - d_be_m.ib) / (2.0 * h), p * s.gib_be, "ib/vbe"),
                    ((d_bc.ib - d_bc_m.ib) / (2.0 * h), p * s.gib_bc, "ib/vbc"),
                    ((d_be.ic - d_be_m.ic) / (2.0 * h), p * s.gic_be, "ic/vbe"),
                    ((d_bc.ic - d_bc_m.ic) / (2.0 * h), p * s.gic_bc, "ic/vbc"),
                    ((d_be.ie - d_be_m.ie) / (2.0 * h), p * s.gie_be, "ie/vbe"),
                    ((d_bc.ie - d_bc_m.ie) / (2.0 * h), p * s.gie_bc, "ie/vbc"),
                ] {
                    let scale = ana.abs().max(num.abs()).max(1e-9);
                    assert!(
                        (num - ana).abs() / scale < 1e-4,
                        "{what} at {vbe}/{vbc}: numeric {num:e} vs stamped {ana:e}"
                    );
                }
            }
        }
    }

    /// Forward-active behaviour: a germanium PNP biased at a couple of hundred
    /// microamps sits near a 0.2 V `Vbe` and delivers `βF` times its base
    /// current — the two facts the Rangemaster's operating point rests on.
    #[test]
    fn a_germanium_pnp_biases_where_germanium_does() {
        let q = ge_pnp();
        // 0.2 V emitter–base forward, collector well reverse-biased.
        let s = q.stamp(0.0, 0.2, -4.0);
        // For a PNP the external emitter current is negative (it flows *into*
        // the emitter terminal), so compare magnitudes.
        let ie = s.ie.abs();
        assert!((1e-4..1e-3).contains(&ie), "Ie = {ie:e} A at Vbe 0.2 V");
        // Slightly *above* βF: the reverse junction's −Is leaks out of the base
        // in the forward-active region, so the current gain a germanium device
        // measures is the datasheet βF plus its leakage. That is the same
        // mechanism that makes real germanium bias drift with temperature.
        let beta = s.ic.abs() / s.ib.abs();
        assert!((95.0..110.0).contains(&beta), "beta = {beta}");
    }

    /// Saturation is a real region, not a clamp: pull the collector back toward
    /// the base and the reverse junction steals the collector current away.
    #[test]
    fn the_transistor_saturates_when_the_collector_swings_back() {
        let q = ge_pnp();
        let active = q.stamp(0.0, 0.2, -4.0).ic.abs();
        // Within 20 mV of the base — where a hard-driven collector actually
        // lands, and eight thermal voltages into the reverse exponential.
        let saturated = q.stamp(0.0, 0.2, 0.18).ic.abs();
        assert!(
            saturated < 0.5 * active,
            "saturation must fold the collector current: {saturated:e} vs {active:e}"
        );
    }

    /// The 3×3 solve is a solve: reconstruct `b` from the answer.
    #[test]
    fn solve3_solves() {
        // Deliberately ill-scaled, like the real node matrix.
        let m = [
            [1.9e-3, -2.4e-5, -1e-12],
            [-2.4e-5, 18.05, -3e-9],
            [3.1e-3, -3.1e-3, 1e-4],
        ];
        let b = [1.0e-4, 0.46, 2.0e-4];
        let x = solve3(m, b).expect("non-singular");
        for r in 0..3 {
            let got: f64 = (0..3).map(|c| m[r][c] * x[c]).sum();
            let scale = b[r].abs().max(1e-12);
            assert!(
                (got - b[r]).abs() / scale < 1e-9,
                "row {r}: {got} vs {}",
                b[r]
            );
        }
    }

    /// A singular matrix returns `None` rather than `NaN` — the caller's cue to
    /// keep its previous iterate (RT rule 7: no NaN may escape a node).
    #[test]
    fn solve3_refuses_a_singular_matrix() {
        let m = [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [1.0, 1.0, 1.0]];
        assert!(solve3(m, [1.0, 2.0, 3.0]).is_none());
    }
}
