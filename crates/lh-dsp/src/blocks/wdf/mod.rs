//! Wave Digital Filter (WDF) primitives — the white-box circuit-modelling
//! substrate (white paper §6 deep water, first topic: the Tube Screamer
//! clipping stage; see PRD 020 / ADR 028).
//!
//! # Why the wave domain
//!
//! The rest of the drive family is *memoryless* waveshaping: a static curve
//! `y = f(x)` plus filters. That cannot capture a real clipper's soul — the
//! way an RC network and a diode's junction interact so the clipping threshold
//! moves with frequency and transient. WDF discretizes the actual circuit: it
//! rewrites each element in terms of **wave variables** `a = v + R·i` (incident)
//! and `b = v − R·i` (reflected), where `R` is a per-port *reference
//! resistance*. Linear elements become trivial one-ports; a single
//! nonlinearity (the diode) sits at the *root* of a tree of adaptors, which
//! present it a Thévenin equivalent — one incident wave `a` and one resistance
//! `R`. The nonlinearity then only has to solve a scalar equation on its own
//! v–i curve. Change the circuit by changing component values, not by hand-
//! tuning a curve — that is the white box.
//!
//! # What lives here
//!
//! The genuinely reusable, non-trivial pieces: the bilinear [`Capacitor`]
//! one-port, the antiparallel [`DiodePair`] root, the asymmetric [`AsymDiode`]
//! root, the [`parallel_root`] adaptor helpers, and the [`omega`] module — the
//! Wright omega approximation that lets `DiodePair` be evaluated in closed form
//! instead of iterated. A specific circuit (e.g. the Screamer's shunt clipper
//! in `drive/screamer.rs`) composes them into straight-line per-sample code —
//! no boxed tree, no dynamic dispatch, no allocation on the audio thread.

pub mod omega;

/// Bilinear-transform (trapezoidal) capacitor as a WDF one-port.
///
/// Reference resistance `R = T/(2C) = 1/(2·C·fs)`; the reflected wave is a pure
/// unit delay of the incident wave (`b[n] = a[n−1]`), so the element's entire
/// state is the last incident wave it was handed.
pub struct Capacitor {
    r: f32,
    /// `a[n−1]` — the incident wave stored from last sample; also this port's
    /// reflected wave this sample.
    state: f32,
}

impl Capacitor {
    /// A capacitor of `farads` at the given sample rate.
    pub fn new(farads: f32, sample_rate: f32) -> Self {
        debug_assert!(farads > 0.0 && sample_rate > 0.0);
        Self {
            r: 1.0 / (2.0 * farads * sample_rate),
            state: 0.0,
        }
    }

    /// The port reference resistance `R = T/(2C)`.
    #[inline]
    pub fn resistance(&self) -> f32 {
        self.r
    }

    /// The port conductance `G = 1/R = 2·C·fs`.
    #[inline]
    pub fn conductance(&self) -> f32 {
        1.0 / self.r
    }

    /// The wave this port reflects toward the adaptor this sample.
    #[inline]
    pub fn reflected(&self) -> f32 {
        self.state
    }

    /// Store the adaptor's incident wave `b` for this port; it becomes next
    /// sample's reflected wave. Denormals are flushed (RT rule 7 — a decaying
    /// feedback state must not sink into denormal territory).
    #[inline]
    pub fn set_incident(&mut self, b: f32) {
        self.state = if b.abs() < 1e-25 { 0.0 } else { b };
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Two identical diodes in antiparallel (opposite polarity) as a WDF nonlinear
/// **root**: the symmetric soft clipper at the heart of a Tube-Screamer-style
/// pedal (`i(v) = 2·Is·sinh(v / (n·Vt))`).
///
/// Given the incident wave `a` the linear subtree presents and the port
/// resistance `R` it sees, [`solve`](Self::solve) returns the diode voltage `v`
/// (which is also the clipping node's voltage — the useful output) and the
/// reflected wave `b = 2v − a`.
///
/// # How it is solved
///
/// Two paths, both RT-safe, both always compiled:
///
/// - [`solve`](Self::solve) — **closed form**, no iteration. Werner et al.
///   rearrange the root equation into two evaluations of Wright's `ω`
///   ([`omega`]), by treating the pair as "whichever diode conducts, plus a
///   negligible correction from the other". That last step makes it a very
///   accurate *approximation*, not an identity, which is why the Newton path
///   stays.
/// - [`solve_newton`](Self::solve_newton) — the **oracle**: the damped `f64`
///   Newton iteration this replaced. It solves the true equation to `1e-10`, so
///   the tests can measure exactly what the closed form costs in accuracy
///   (worst case ~30 µV on the node, harmonics within 0.001 dB) instead of
///   taking the port on faith.
///
/// # Reference
///
/// K. Werner et al., *"An Improved and Generalized Diode Clipper Model for Wave
/// Digital Filters"*, DAFx-16 — eqn (39), the antiparallel-pair form. Structure
/// follows `chowdsp_wdf`'s `wdft_nonlinearities.h` (BSD-3).
pub struct DiodePair {
    /// Reverse saturation current `Is` (A).
    is: f32,
    /// The thermal scale `n·Vt` (ideality × thermal voltage), in volts.
    vt_n: f32,
    /// `1 / (n·Vt)`, precomputed for the hot path.
    one_over_vt: f32,
    /// `ln(R·Is / (n·Vt))` — the *only* term of the closed form that depends on
    /// the port resistance.
    log_r_is_over_vt: f32,
    /// The port resistance `log_r_is_over_vt` was computed for. Starts `NaN`,
    /// which compares unequal to everything, so the first solve always fills it.
    r_cached: f32,
    /// Warm start for [`solve_newton`](Self::solve_newton): the voltage it
    /// solved last sample. Audio is continuous, so that is almost always 1–3
    /// steps from the new root. Unused by the closed form, which is stateless.
    v_newton: f64,
}

/// Newton iteration ceiling. Warm-started real audio needs 1–3; a cold start
/// from a slammed input converges in well under this (damped, see below).
const MAX_ITERS: usize = 16;
/// Convergence tolerance on the diode voltage, in volts.
const TOL: f64 = 1e-10;

impl DiodePair {
    /// `is` = saturation current (A), `n` = ideality factor, `vt` = thermal
    /// voltage (V). 1N4148 ≈ `is 2.52e-9, n 1.75, vt 25.85e-3`.
    pub fn new(is: f32, n: f32, vt: f32) -> Self {
        let vt_n = n * vt;
        Self {
            is,
            vt_n,
            one_over_vt: 1.0 / vt_n,
            log_r_is_over_vt: 0.0,
            r_cached: f32::NAN,
            v_newton: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.v_newton = 0.0;
    }

    /// Solve the root for incident wave `a` at port resistance `r`. Returns
    /// `(v, b)`: the diode/node voltage and the reflected wave `2v − a`.
    ///
    /// **RT-safe:** no allocation, no iteration, no `std::exp`/`std::log` on
    /// the hot path — a fixed handful of polynomial evaluations and two
    /// divides. The `f64` `ln` fires only when `r` *changes*; for a fixed
    /// circuit (`screamer`) that is once, at `prepare`.
    #[inline]
    pub fn solve(&mut self, a: f32, r: f32) -> (f32, f32) {
        if r != self.r_cached {
            self.r_cached = r;
            // `R·Is/Vt` is ~1e-4 and its log lands directly in a voltage, so the
            // budget term is derived in `f64` and stored down — precision where
            // it is cheap (RT-safe: libm, no allocation, no lock).
            self.log_r_is_over_vt =
                ((f64::from(r) * f64::from(self.is)) / f64::from(self.vt_n)).ln() as f32;
        }

        // Werner eqn (39). With `λ = sign(a)`, `ω(L + λa/Vt)` is the conducting
        // branch and `ω(L − λa/Vt)` the reverse one (tiny, but it is what makes
        // the crossover region smooth rather than kinked). `copysign` is the
        // branchless way to spell `sign`; `a = ±0` lands on `la = ±0`, where the
        // two branches cancel and `v = a` either way.
        let lambda = 1.0f32.copysign(a);
        let la = lambda * a * self.one_over_vt;
        let l = self.log_r_is_over_vt;
        let v = a - self.vt_n * lambda * (omega::omega(l + la) - omega::omega(l - la));
        let v = if v.abs() < 1e-25 { 0.0 } else { v };
        (v, 2.0 * v - a)
    }

    /// The **reference** solve: warm-started, damped `f64` Newton on
    /// `f(v) = v + R·i(v) − a`, converged to [`TOL`].
    ///
    /// Kept permanently, not as legacy: [`solve`](Self::solve) is a closed-form
    /// *approximation*, and this is the ground truth its golden tests and
    /// benchmarks measure against. Production code should call
    /// [`solve`](Self::solve) — this is ~13× the cost back to back, and ~2.4×
    /// the whole pedal once the rest of the circuit is counted.
    ///
    /// **RT-safe** as well: fixed iteration ceiling, no allocation. The Newton
    /// step is *damped* (capped at `10·n·Vt` per iteration) so a cold, slammed
    /// input cannot overshoot into the stiff exponential and stall — real
    /// convergence near the root is undamped and quadratic.
    #[inline]
    pub fn solve_newton(&mut self, a: f32, r: f32) -> (f32, f32) {
        let a = f64::from(a);
        let r = f64::from(r);
        let two_is = 2.0 * f64::from(self.is);
        let vt_n = f64::from(self.vt_n);
        let dv_max = 10.0 * vt_n;

        let mut v = self.v_newton;
        for _ in 0..MAX_ITERS {
            // `u` stays sane thanks to damping; the clamp is pure overflow
            // paranoia for a pathological caller.
            let u = (v / vt_n).clamp(-60.0, 60.0);
            let e = u.exp();
            let einv = 1.0 / e;
            let sinh = 0.5 * (e - einv);
            let cosh = 0.5 * (e + einv);
            let i = two_is * sinh;
            let f = v + r * i - a;
            let fp = 1.0 + r * two_is / vt_n * cosh;
            // Damped step: capped so a cold, slammed input can't overshoot into
            // the stiff exponential. Near the root the cap never bites, so
            // convergence stays undamped (quadratic).
            let dv = (f / fp).clamp(-dv_max, dv_max);
            v -= dv;
            if dv.abs() < TOL {
                break;
            }
        }

        self.v_newton = v;
        let vf = v as f32;
        let vf = if vf.abs() < 1e-25 { 0.0 } else { vf };
        (vf, 2.0 * vf - a as f32)
    }
}

/// Antiparallel diode clipper with **asymmetric** branch counts: `m` identical
/// diodes in series conducting one way, `k` the other. Its v–i curve is
///
/// ```text
/// i(v) = Is·(exp(v / (m·nVt)) − exp(−v / (k·nVt)))
/// ```
///
/// — monotonic (so Newton converges), and it reduces to [`DiodePair`]'s
/// `2·Is·sinh(v/nVt)` exactly when `m = k = 1`. When `m ≠ k` the two clipping
/// thresholds skew apart, so a symmetric input clips asymmetrically and grows
/// **even** harmonics — the Boss SD-1's signature (2 diodes one way, 1 the
/// other) and the difference from a Tube Screamer's matched pair. This is the
/// WDF nonlinear **root** for a feedback-topology overdrive; see
/// `drive/sd1.rs`.
pub struct AsymDiode {
    is: f64,
    /// Forward thermal scale `m·n·Vt`.
    vt_f: f64,
    /// Reverse thermal scale `k·n·Vt`.
    vt_r: f64,
    /// Warm start: the voltage solved last sample (continuous audio → 1–3 steps).
    v: f64,
}

impl AsymDiode {
    /// `is`/`n`/`vt` are the *single*-diode parameters (1N4148 ≈
    /// `is 2.52e-9, n 1.75, vt 25.85e-3`); `m_fwd`/`m_rev` are how many sit in
    /// series each way (SD-1 ≈ `2` / `1`).
    pub fn new(is: f32, n: f32, vt: f32, m_fwd: f32, m_rev: f32) -> Self {
        let vt_n = (n * vt) as f64;
        Self {
            is: is as f64,
            vt_f: m_fwd as f64 * vt_n,
            vt_r: m_rev as f64 * vt_n,
            v: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.v = 0.0;
    }

    /// Solve the root for incident wave `a` at port resistance `r`. Returns
    /// `(v, b)` — the clip-node voltage and the reflected wave `2v − a`. RT-safe
    /// exactly like [`DiodePair::solve`]: damped, warm-started `f64` Newton with
    /// a fixed ceiling and an exp clamp (bounded and finite by construction).
    #[inline]
    pub fn solve(&mut self, a: f32, r: f32) -> (f32, f32) {
        let a = a as f64;
        let r = r as f64;
        let is = self.is;
        let (vt_f, vt_r) = (self.vt_f, self.vt_r);
        // Damp against the stiffer (smaller-scale) branch so a cold, slammed
        // input can't overshoot into the exponential and stall.
        let dv_max = 10.0 * vt_f.min(vt_r);

        let mut v = self.v;
        for _ in 0..MAX_ITERS {
            let uf = (v / vt_f).clamp(-60.0, 60.0);
            let ur = (-v / vt_r).clamp(-60.0, 60.0);
            let ef = uf.exp();
            let er = ur.exp();
            let i = is * (ef - er);
            // di/dv = Is·(ef/vt_f + er/vt_r) — both terms ≥ 0, so i is monotonic.
            let ip = is * (ef / vt_f + er / vt_r);
            let f = v + r * i - a;
            let fp = 1.0 + r * ip;
            let dv = (f / fp).clamp(-dv_max, dv_max);
            v -= dv;
            if dv.abs() < TOL {
                break;
            }
        }

        self.v = v;
        let vf = v as f32;
        let vf = if vf.abs() < 1e-25 { 0.0 } else { vf };
        (vf, 2.0 * vf - a as f32)
    }
}

/// Incident wave into the adapted (reflection-free) root of a parallel adaptor,
/// and the resistance the root sees, from the linear ports' `(conductance,
/// reflected-wave)` pairs.
///
/// At a parallel node every port shares one voltage, so the root sees the
/// conductance-weighted average of the others' waves,
/// `a = (Σ Gₖ·aₖ) / (Σ Gₖ)`, behind `R = 1 / (Σ Gₖ)`. Making the root
/// reflection-free (its own conductance set to `Σ Gₖ`) is what breaks the
/// delay-free loop so the tree is computable in one pass.
///
/// `ports` is borrowed from a caller-owned stack array (e.g.
/// `&[(g0, a0), (g1, a1)]`) — no heap, RT-safe.
#[inline]
pub fn parallel_root(ports: &[(f32, f32)]) -> (f32, f32) {
    let mut g_sum = 0.0f32;
    let mut weighted = 0.0f32;
    for &(g, a) in ports {
        g_sum += g;
        weighted += g * a;
    }
    (weighted / g_sum, 1.0 / g_sum)
}

/// Like [`parallel_root`] but with an external current source `i_src` injected
/// into the node: the root's Thévenin voltage rises by `i_src · R`, i.e.
/// `a = (Σ Gₖ·aₖ + i_src) / Σ Gₖ` behind the same `R = 1 / Σ Gₖ`. This is how an
/// ideal op-amp's forced feedback current drives a diode clipper — the
/// virtual-short reduction of a feedback-topology overdrive (`drive/sd1.rs`).
/// `parallel_root(ports)` is exactly the `i_src = 0` case.
#[inline]
pub fn parallel_root_with_source(ports: &[(f32, f32)], i_src: f32) -> (f32, f32) {
    let mut g_sum = 0.0f32;
    let mut weighted = 0.0f32;
    for &(g, a) in ports {
        g_sum += g;
        weighted += g * a;
    }
    ((weighted + i_src) / g_sum, 1.0 / g_sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1N4148-ish pair for the solver tests.
    fn diode() -> DiodePair {
        DiodePair::new(2.52e-9, 1.75, 25.85e-3)
    }

    /// The **oracle's** contract: it really does solve `a = v + R·i(v)`, and
    /// obeys the wave identity `b = 2v − a`. Everything else about the root's
    /// accuracy is measured relative to this.
    ///
    /// Note the residual is checked on `solve_newton`, not `solve`: the closed
    /// form is an approximation, and near the knee `R·di/dv` is ~130, so a
    /// 40 µV voltage error shows up as a ~5 mV residual. Voltage is the
    /// meaningful unit here (it is what the circuit outputs), so the closed
    /// form is judged in volts by `solve_matches_the_newton_oracle`.
    #[test]
    fn newton_oracle_satisfies_the_diode_equation() {
        let r = 2200.0f32;
        for &a in &[-40.0, -5.0, -0.5, -0.01, 0.0, 0.01, 0.5, 5.0, 40.0] {
            let mut d = diode();
            let (v, b) = d.solve_newton(a, r);
            assert!(v.is_finite() && b.is_finite());
            // Residual of a = v + R·i(v), computed in f64.
            let vt_n = 1.75 * 25.85e-3f64;
            let i = 2.0 * 2.52e-9f64 * (v as f64 / vt_n).sinh();
            let residual = v as f64 + r as f64 * i - a as f64;
            assert!(residual.abs() < 1e-4, "a={a}: residual {residual}");
            assert!((b - (2.0 * v - a)).abs() < 1e-4, "b identity a={a}");
        }
    }

    /// **The golden**: the closed form must be an equivalent replacement for
    /// the oracle, not a new sound. Measured across the incident waves a
    /// slammed drive stage actually produces and across the port resistances
    /// the family's clipper networks present.
    ///
    /// The bound is pinned from measurement — the error is bounded by
    /// construction (eqn (39)'s crossover approximation plus [`omega`]'s
    /// polynomial ladder), so a regression in either shows up as a widening
    /// here rather than as a mystery in someone's ears.
    #[test]
    fn solve_matches_the_newton_oracle() {
        /// Pinned worst-case node-voltage disagreement, in volts.
        const MAX_DV: f32 = 5e-5;

        let mut worst = (0.0f32, 0.0f32, 0.0f32);
        for &r in &[1_000.0f32, 2_200.0, 4_700.0, 47_000.0] {
            // Dense through the knee (±1.5 V, where the curve bends), sparse
            // out to the levels a slammed op-amp stage delivers.
            let grid = (0..3_001)
                .map(|k| -1.5 + k as f32 * 3.0 / 3_000.0)
                .chain((0..2_001).map(|k| -50.0 + k as f32 * 100.0 / 2_000.0));
            for a in grid {
                let (v_omega, _) = diode().solve(a, r);
                let (v_newton, _) = diode().solve_newton(a, r);
                let dv = (v_omega - v_newton).abs();
                if dv > worst.0 {
                    worst = (dv, a, r);
                }
            }
        }
        assert!(
            worst.0 < MAX_DV,
            "worst |Δv| = {:.3e} V at a = {:.4}, R = {} (bound {MAX_DV:e})",
            worst.0,
            worst.1,
            worst.2
        );
    }

    /// The *audible* form of the golden. Voltage error is the engineering
    /// measure; what actually has to be indistinguishable is the harmonic
    /// series a clipped sine comes out with. Run at 192 kHz — the 4×
    /// oversampled rate the root really runs at — over exactly 10 cycles of
    /// 200 Hz so every harmonic lands on a DFT bin centre.
    #[test]
    fn solve_matches_the_newton_oracle_spectrally() {
        const SR: f64 = 4.0 * 48_000.0;
        const F: f64 = 200.0;
        const N: usize = 9_600; // 10 whole cycles

        /// Magnitude of harmonic `h` of `f`, by direct DFT at the bin centre.
        fn harmonic(buf: &[f32], h: usize) -> f64 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, s) in buf.iter().enumerate() {
                let ph = std::f64::consts::TAU * F * h as f64 * i as f64 / SR;
                re += f64::from(*s) * ph.cos();
                im -= f64::from(*s) * ph.sin();
            }
            2.0 * re.hypot(im) / buf.len() as f64
        }

        let r = 2_200.0f32;
        let (mut o, mut n) = (diode(), diode());
        let (mut y_o, mut y_n) = (Vec::with_capacity(N), Vec::with_capacity(N));
        for k in 0..N {
            let a = 5.0 * (std::f64::consts::TAU * F * k as f64 / SR).sin() as f32;
            y_o.push(o.solve(a, r).0);
            y_n.push(n.solve_newton(a, r).0);
        }

        // A symmetric clipper puts nothing in the even harmonics, so their
        // ratio is noise over noise; the floor (−120 dBc) makes the comparison
        // "equal or both buried" instead of dividing two rounding errors.
        let floor = 1e-6 * harmonic(&y_n, 1);
        for h in 1..=10 {
            let d = 20.0 * ((harmonic(&y_o, h) + floor) / (harmonic(&y_n, h) + floor)).log10();
            assert!(d.abs() < 0.1, "harmonic {h}: {d:.4} dB apart");
        }
    }

    /// Antiparallel diodes are symmetric: `v(−a) = −v(a)`.
    #[test]
    fn solve_is_symmetric() {
        let r = 2200.0f32;
        for &a in &[0.3, 1.0, 8.0, 30.0] {
            let (vp, _) = diode().solve(a, r);
            let (vn, _) = diode().solve(-a, r);
            assert!((vp + vn).abs() < 1e-5, "a={a}: {vp} vs {vn}");
        }
    }

    /// The clip is a soft ceiling: the node voltage saturates far below the
    /// input as `a` grows (the whole point of a clipper).
    #[test]
    fn solve_saturates() {
        let r = 2200.0f32;
        let (v_small, _) = diode().solve(0.05, r);
        let (v_big, _) = diode().solve(50.0, r);
        // Small signal is near-unity (barely clipped), big signal is clamped
        // near the diode knee (well under a volt).
        assert!(v_small > 0.02, "small {v_small}");
        assert!(v_big < 0.9 && v_big > 0.4, "big {v_big}");
        assert!(v_big < v_small * 20.0, "should compress hard");
    }

    /// Warm-started continuity of the oracle: solving a slowly moving `a` never
    /// blows up and stays consistent with cold solves. (The closed form is
    /// stateless, so this can only bite the Newton path.)
    #[test]
    fn newton_warm_start_matches_cold() {
        let r = 2200.0f32;
        let mut warm = diode();
        for k in 0..200 {
            let a = 10.0 * (k as f32 * 0.03).sin();
            let (vw, _) = warm.solve_newton(a, r);
            let (vc, _) = diode().solve_newton(a, r); // fresh, cold
            assert!((vw - vc).abs() < 1e-4, "k={k}: warm {vw} cold {vc}");
        }
    }

    /// The closed form carries no state between samples, so a running solve and
    /// a cold one agree *exactly* — which is also why it needs no reset.
    #[test]
    fn solve_is_stateless() {
        let r = 4700.0f32;
        let mut running = diode();
        for k in 0..200 {
            let a = 10.0 * (k as f32 * 0.03).sin();
            assert_eq!(running.solve(a, r).0, diode().solve(a, r).0, "k={k}");
        }
    }

    /// Changing the port resistance mid-stream must re-derive the closed form's
    /// `R`-dependent term (an adaptor with a pot in it moves `R` while audio is
    /// running), and must land where a fresh solver at that `R` lands.
    #[test]
    fn solve_tracks_a_changing_port_resistance() {
        let mut d = diode();
        for &r in &[2_200.0f32, 47_000.0, 1_000.0, 2_200.0, 220_000.0] {
            for &a in &[-8.0f32, -0.4, 0.4, 8.0] {
                assert_eq!(d.solve(a, r).0, diode().solve(a, r).0, "a={a} r={r}");
            }
        }
    }

    /// A slammed, alternating input must stay finite and bounded (RT rule 7).
    #[test]
    fn solve_bounded_when_slammed() {
        let r = 4700.0f32;
        let mut d = diode();
        for k in 0..1000 {
            let a = if k % 2 == 0 { 1.0e6 } else { -1.0e6 };
            let (v, b) = d.solve(a, r);
            assert!(v.is_finite() && b.is_finite());
            assert!(v.abs() < 1.5, "k={k}: v={v}");
        }
    }

    #[test]
    fn capacitor_port_resistance() {
        // R = T/(2C) = 1/(2·C·fs).
        let c = Capacitor::new(47e-9, 48_000.0);
        let expected = 1.0 / (2.0 * 47e-9 * 48_000.0);
        assert!((c.resistance() - expected).abs() / expected < 1e-5);
        assert!((c.conductance() - 1.0 / expected).abs() / (1.0 / expected) < 1e-5);
    }

    /// A source–resistor–capacitor divider (no diode) must settle to the DC
    /// the resistances dictate: with the capacitor open at DC, the node sees
    /// the full source EMF.
    #[test]
    fn parallel_root_rc_settles_to_dc() {
        let sr = 48_000.0f32;
        let mut cap = Capacitor::new(100e-9, sr);
        let g_src = 1.0 / 1000.0; // 1 kΩ source
        let e = 0.7f32; // constant EMF
        let mut v = 0.0f32;
        for _ in 0..20_000 {
            let a1 = cap.reflected();
            let (a_root, _r_root) = parallel_root(&[(g_src, e), (cap.conductance(), a1)]);
            // No diode: an open root reflects its incident unchanged, so the
            // node voltage equals the incident wave. Back-propagate to the
            // capacitor exactly as the clipper does: b_cap = 2·v − a_cap.
            v = a_root;
            cap.set_incident(2.0 * v - a1);
        }
        // Capacitor fully charged, no current: node = source EMF.
        assert!((v - e).abs() < 1e-3, "settled {v}, want {e}");
    }

    fn asym(m: f32, k: f32) -> AsymDiode {
        AsymDiode::new(2.52e-9, 1.75, 25.85e-3, m, k)
    }

    /// The matched case `m = k = 1` must reproduce the antiparallel
    /// `DiodePair` (`2·Is·sinh`) — the asymmetric solver is a strict superset.
    ///
    /// This is a claim about the two *equations*, so it is checked against the
    /// pair's Newton oracle: both sides then solve their equation exactly and
    /// the tolerance can stay tight. (`DiodePair::solve`'s closed form is an
    /// approximation and would only muddy the comparison with its own ~40 µV.)
    #[test]
    fn asym_diode_matches_diode_pair_when_symmetric() {
        let r = 2200.0f32;
        for &a in &[-30.0, -3.0, -0.3, 0.0, 0.3, 3.0, 30.0] {
            let (vp, bp) = DiodePair::new(2.52e-9, 1.75, 25.85e-3).solve_newton(a, r);
            let (va, ba) = asym(1.0, 1.0).solve(a, r);
            assert!((vp - va).abs() < 1e-6, "a={a}: pair {vp} asym {va}");
            assert!((bp - ba).abs() < 1e-6, "a={a}: b pair {bp} asym {ba}");
        }
    }

    /// The returned root satisfies `a = v + R·i(v)` for the asymmetric curve.
    #[test]
    fn asym_diode_solves_its_equation() {
        let r = 4700.0f32;
        let vt_n = 1.75 * 25.85e-3f64;
        for &a in &[-20.0, -1.0, -0.02, 0.02, 1.0, 20.0] {
            let (v, b) = asym(2.0, 1.0).solve(a, r);
            assert!(v.is_finite() && b.is_finite());
            let i = 2.52e-9f64 * ((v as f64 / (2.0 * vt_n)).exp() - (-(v as f64) / vt_n).exp());
            let residual = v as f64 + r as f64 * i - a as f64;
            assert!(residual.abs() < 1e-4, "a={a}: residual {residual}");
        }
    }

    /// `2` diodes one way, `1` the other: the two-diode (forward) side needs
    /// ~twice the voltage to conduct, so it clips *later* → `v(−a) ≠ −v(a)`,
    /// the source of even harmonics. A matched pair (above) cancels this.
    #[test]
    fn asym_diode_is_asymmetric() {
        let r = 4700.0f32;
        for &a in &[1.0f32, 5.0, 30.0] {
            let (vp, _) = asym(2.0, 1.0).solve(a, r);
            let (vn, _) = asym(2.0, 1.0).solve(-a, r);
            assert!(
                (vp + vn).abs() > 0.05,
                "asymmetric clip must not cancel: a={a} vp={vp} vn={vn}"
            );
            assert!(vp > -vn, "the 2-diode side clips later: {vp} vs {}", -vn);
        }
    }

    /// A slammed, alternating input stays finite and bounded (RT rule 7).
    #[test]
    fn asym_diode_bounded_when_slammed() {
        let r = 4700.0f32;
        let mut d = asym(2.0, 1.0);
        for k in 0..1000 {
            let a = if k % 2 == 0 { 1.0e6 } else { -1.0e6 };
            let (v, b) = d.solve(a, r);
            assert!(v.is_finite() && b.is_finite());
            // The 2-diode forward side clamps near 2·nVt·ln(i/Is) ≈ 2.3 V under
            // this (unphysical) slam; bounded means "no runaway", not sub-volt.
            assert!(v.abs() < 3.0, "k={k}: v={v}");
        }
    }

    /// The current-injected parallel root: zero source reproduces
    /// [`parallel_root`] bit-for-bit, and a positive current lifts the node
    /// voltage by exactly `i·R` while leaving the resistance untouched.
    #[test]
    fn parallel_root_source_injects_current() {
        let ports = [(1.0 / 1000.0, 0.3f32), (1.0 / 2200.0, -0.1f32)];
        let (a0, r0) = parallel_root(&ports);
        assert_eq!((a0, r0), parallel_root_with_source(&ports, 0.0));
        let (a1, r1) = parallel_root_with_source(&ports, 1e-3);
        assert!((r1 - r0).abs() < 1e-9, "resistance unchanged by the source");
        assert!((a1 - (a0 + 1e-3 * r0)).abs() < 1e-6, "node rises by i·R");
    }
}
