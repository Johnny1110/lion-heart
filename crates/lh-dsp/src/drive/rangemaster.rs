//! **rangemaster** — the Dallas Rangemaster treble booster: one germanium PNP
//! transistor, solved as a **three-node Ebers–Moll circuit** (PRD 033).
//! The second pedal of Tone Revolution phase 05, and the first in the family
//! whose nonlinearity is the transistor itself rather than a diode.
//!
//! # Why this one needs a real device model
//!
//! [`super::big_muff`]'s transistor is linearised to a bare gain `A = −Rc/Re`,
//! because everything that clips there happens in the *feedback network*.
//! Nothing clips in a Rangemaster except the transistor. Its two mechanisms are
//! the two junctions:
//!
//! - **cutoff** — the emitter–base junction turning off as the signal pulls the
//!   base away from its bias, which chops one polarity abruptly;
//! - **saturation** — the collector–base junction turning *on* as the collector
//!   swings back toward the base, which squashes the other polarity gently.
//!
//! They are not mirror images, which is why one transistor grows a fat second
//! harmonic where a diode pair grows none. A curve can imitate the shape; only
//! the device model gets the way the shape *moves* when the operating point
//! drifts — and it drifts constantly, because the 47 µF emitter bypass takes
//! about a second to recover from a hard chord.
//!
//! A bipolar transistor is a two-port nonlinearity (two exponentials sharing a
//! base), so unlike every diode in this family it cannot be a WDF root at all.
//! [`crate::blocks::transistor::Bjt`] supplies the linearisation and this file
//! runs the nodal Newton over it: three unknowns (base, emitter, collector),
//! three KCL equations, a 3×3 solve per iteration.
//!
//! # Germanium is not silicon with different numbers
//!
//! An OC44's saturation current is ~1e-7 A against a silicon 2N3904's ~1e-14 —
//! seven decades, which is *why* a germanium stage biases at a 0.2 V `Vbe`,
//! conducts softly, leaks, and drifts. Parameters here are germanium's, not the
//! reference implementation's (see PRD 033 §1.2); the same policy ADR 033 set
//! for diode menus, one device class along.
//!
//! # The treble boost
//!
//! `C1` (5 nF stock) into the stage's ~10 kΩ input impedance is a 3 kHz
//! high-pass — that, and nothing else, is what makes this a *treble* booster
//! rather than a booster. The **Range** knob is that capacitor: 2.2 nF at 0,
//! the stock 5 nF at noon, 47 nF ("full range") at 10. Because the input
//! impedance is `β·re` and `re` moves with the emitter current, the corner
//! shifts a little as the pedal is driven — which a fixed input filter in front
//! of a curve cannot do.
//!
//! Faceplate: **Boost / Range / Level**.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use crate::blocks::transistor::{
    Bjt, NODAL_DV_MAX_VT, NODAL_MAX_ITERS, NODAL_TOL, Polarity, solve3,
};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("boost", "Boost", 5.0, 20.0),
    knob("range", "Range", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "rangemaster",
    name: "Rangemaster",
    params: &PARAMS,
};

// --- the netlist (values off the schematic) ---

/// Supply rail. The real pedal is *positive ground* with a −9 V rail and a PNP;
/// this is the same circuit re-referenced so the rail is positive, which is a
/// change of sign convention and nothing else.
const V_PLUS: f64 = 9.0;
/// Base bias divider: `R1` to ground, `R2` to the rail.
const R1: f32 = 470e3;
const R2: f32 = 68e3;
/// Emitter degeneration, bypassed by `C3`.
const R3: f32 = 3.9e3;
/// The 47 µF bypass. Its corner with `R3` is 0.87 Hz, so at audio the emitter
/// is a short to the rail — but under a hard chord the *average* emitter
/// current shifts and this capacitor takes about a second to follow, which is
/// the germanium booster's bloom.
const C3: f32 = 47e-6;
/// Collector load: the Boost pot's track, output taken from its wiper.
const RV: f32 = 10e3;

/// Input coupling capacitor at the three positions the Range knob interpolates
/// between: a common treble mod, the stock part, and the "full range" mod.
const C_TREBLE: f32 = 2.2e-9;
const C_STOCK: f32 = 5.0e-9;
const C_FULL: f32 = 47e-9;

// OC44-class germanium PNP. `Is` is set by what germanium *does*: a 0.2 V
// emitter–base drop at a couple of hundred microamps, which is a hundred
// nanoamps of saturation current — the same decade as the 1N34A germanium
// diode already in `ts-wdf`'s menu, and seven decades off silicon.
const IS: f32 = 1.0e-7;
const VT: f32 = 25.85e-3;
const BETA_F: f32 = 100.0;
/// Alloy-junction germanium has almost no reverse gain, which is what makes
/// saturation soft rather than a wall.
const BETA_R: f32 = 2.0;

/// Samples between coefficient rebuilds while the Range knob moves.
const REBUILD: usize = 64;
/// Time constant for the Range knob's internal glide.
const GLIDE_MS: f32 = 10.0;
/// Output coupling `C2` (10 nF) into a nominal 1 MΩ amp input.
const OUT_HZ: f32 = 16.0;
/// Calibrated with `default_level_survey`.
const MAKEUP: f32 = 0.176;

/// Boost is the level *into* the stage — the real pedal has no gain control at
/// all (its one knob is the output volume), and is played by rolling the
/// *guitar's* volume back. This is that knob, where our chain can reach it, so
/// its range is what a guitar's volume pot spans: ±24 dB about noon.
#[inline]
fn boost_gain(pos: f32) -> f32 {
    db_to_lin(-24.0 + 4.8 * pos)
}

/// The input capacitor for a Range position. Two geometric segments so the
/// stock 5 nF lands exactly at noon and each half is linear in octaves.
#[inline]
fn range_farads(pos: f32) -> f32 {
    let n = (pos * 0.1).clamp(0.0, 1.0);
    if n <= 0.5 {
        C_TREBLE * (C_STOCK / C_TREBLE).powf(n * 2.0)
    } else {
        C_STOCK * (C_FULL / C_STOCK).powf((n - 0.5) * 2.0)
    }
}

/// Everything about the circuit that changes between one solve and the next:
/// the driving voltage, the two capacitor companions, and their history
/// currents. Bundled so the DC operating-point solve can reuse the same Newton
/// with the capacitors open — `Env::default()` is "both capacitors absent, no
/// source", which is exactly what a DC analysis means.
#[derive(Clone, Copy, Default)]
struct Env {
    vsrc: f64,
    gc1: f64,
    gc3: f64,
    s1: f64,
    s3: f64,
}

pub(super) struct Rangemaster {
    q: Bjt,
    /// Node voltages `[base, emitter, collector]`, warm-started from the last
    /// sample — the whole reason three Newton steps is enough.
    v: [f64; 3],
    /// The DC operating point, solved once at [`Circuit::prepare`] and restored
    /// by [`Circuit::reset`]. Settling it by running silence would cost ten
    /// thousand Newton solves on whichever thread called `reset`.
    op: [f64; 3],
    /// Fixed conductances: bias divider, emitter degeneration, collector load.
    g1: f64,
    g2: f64,
    g3: f64,
    gv: f64,
    /// Bilinear conductances of the two capacitors, and their history currents.
    gc1: f64,
    gc3: f64,
    s1: f64,
    s3: f64,
    range: f32,
    range_target: f32,
    glide: f32,
    os_rate: f32,
    dc: OnePole,
    c_dc: f32,
}

impl Rangemaster {
    pub(super) fn new() -> Self {
        Self {
            q: Bjt::new(IS, VT, BETA_F, BETA_R, Polarity::Pnp),
            v: [0.0; 3],
            op: [0.0; 3],
            g1: 1.0 / f64::from(R1),
            g2: 1.0 / f64::from(R2),
            g3: 1.0 / f64::from(R3),
            gv: 1.0 / f64::from(RV),
            gc1: 0.0,
            gc3: 0.0,
            s1: 0.0,
            s3: 0.0,
            range: 5.0,
            range_target: 5.0,
            glide: 1.0,
            os_rate: 4.0 * 48_000.0,
            dc: OnePole::default(),
            c_dc: 0.0,
        }
    }

    /// Damped nodal Newton on `[Vb, Ve, Vc]`.
    ///
    /// The three KCL equations are
    ///
    /// ```text
    /// base:      Vb(G1+G2+Gc1) − V+·G2 − Gc1·Vs + s1 + ib = 0
    /// emitter:   (Ve−V+)(G3+Gc3) + s3 − ie              = 0
    /// collector: Vc·Gv + ic                             = 0
    /// ```
    ///
    /// with `(ib, ic, ie)` and their six conductances from
    /// [`Bjt::stamp`]. The step is scaled back whole whenever it would move
    /// either junction more than [`NODAL_DV_MAX_VT`] thermal voltages — the
    /// direction is Newton's, the length is not — because a cold start into
    /// `exp(v/Vt)` otherwise overshoots to `e^400` and never returns.
    ///
    /// Taking `&self` and returning the answer (rather than mutating) is what
    /// lets [`Circuit::prepare`]'s DC solve reuse it with the capacitors open.
    #[inline]
    fn newton(&self, start: [f64; 3], env: &Env, iters: usize) -> [f64; 3] {
        let (vsrc, gc1, gc3, s1, s3) = (env.vsrc, env.gc1, env.gc3, env.s1, env.s3);
        let gb = self.g1 + self.g2 + gc1;
        let ge = self.g3 + gc3;
        let lim = NODAL_DV_MAX_VT * self.q.vt();
        let mut x = start;
        for _ in 0..iters {
            let s = self.q.stamp(x[0], x[1], x[2]);
            let f = [
                x[0] * gb - V_PLUS * self.g2 - gc1 * vsrc + s1 + s.ib,
                (x[1] - V_PLUS) * ge + s3 - s.ie,
                x[2] * self.gv + s.ic,
            ];
            let j = [
                [gb + s.gib_be + s.gib_bc, -s.gib_be, -s.gib_bc],
                [-(s.gie_be + s.gie_bc), ge + s.gie_be, s.gie_bc],
                [s.gic_be + s.gic_bc, -s.gic_be, self.gv - s.gic_bc],
            ];
            let Some(mut d) = solve3(j, f) else {
                // Singular Jacobian: keep the previous iterate rather than
                // letting a NaN reach the output (RT rule 7).
                break;
            };
            let worst = (d[0] - d[1]).abs().max((d[0] - d[2]).abs());
            if worst > lim {
                let k = lim / worst;
                for v in &mut d {
                    *v *= k;
                }
            }
            for i in 0..3 {
                x[i] -= d[i];
            }
            if d.iter().all(|v| v.abs() < NODAL_TOL) {
                break;
            }
        }
        x
    }

    /// Re-solve the DC operating point for the current component values, with
    /// both capacitors open. Runs at `prepare` time only — it is allowed to
    /// iterate as long as it likes.
    fn solve_operating_point(&mut self) {
        // A divider-and-a-diode-drop guess, which is within a volt of the
        // answer and saves the damped climb from zero.
        let vb = V_PLUS * self.g2 / (self.g1 + self.g2);
        let guess = [vb, vb + 0.2, 2.0];
        self.op = self.newton(guess, &Env::default(), 400);
    }

    /// One oversampled sample: solve the three nodes, advance both capacitors,
    /// and hand back the collector's swing about its bias.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        let vsrc = f64::from(x);
        let env = Env {
            vsrc,
            gc1: self.gc1,
            gc3: self.gc3,
            s1: self.s1,
            s3: self.s3,
        };
        self.v = self.newton(self.v, &env, NODAL_MAX_ITERS);
        // Trapezoidal companions advance at the solved node voltages.
        self.s1 = 2.0 * self.gc1 * (vsrc - self.v[0]) - self.s1;
        self.s3 = 2.0 * self.gc3 * (V_PLUS - self.v[1]) - self.s3;
        (self.v[2] - self.op[2]) as f32
    }

    /// Glide the Range knob and rebuild `C1`'s companion when it has actually
    /// moved. The history current scales with the capacitance (it is
    /// `C·dv/dt`-ish), so rescaling it keeps the node voltage continuous across
    /// the rebuild instead of stepping it.
    #[inline]
    fn retune(&mut self) {
        let d = self.range_target - self.range;
        if d.abs() < 1e-4 {
            return;
        }
        self.range += d * self.glide;
        let gc1 = 2.0 * f64::from(range_farads(self.range)) * f64::from(self.os_rate);
        if self.gc1 > 0.0 {
            self.s1 *= gc1 / self.gc1;
        }
        self.gc1 = gc1;
    }
}

impl Circuit for Rangemaster {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.os_rate = os_rate;
        self.gc1 = 2.0 * f64::from(range_farads(self.range)) * f64::from(os_rate);
        self.gc3 = 2.0 * f64::from(C3) * f64::from(os_rate);
        self.glide = 1.0 - (-(REBUILD as f32) / (os_rate * GLIDE_MS * 1e-3)).exp();
        self.c_dc = lp_coeff(OUT_HZ, base_rate);
        self.solve_operating_point();
        self.reset();
    }

    fn reset(&mut self) {
        // Both capacitors start at their DC equilibrium: a trapezoidal
        // companion carrying no current has `s = G·v`.
        self.v = self.op;
        self.s1 = self.gc1 * (0.0 - self.op[0]);
        self.s3 = self.gc3 * (V_PLUS - self.op[1]);
        self.dc.reset();
    }

    fn set_trim(&mut self, value: f32) {
        self.range_target = value.clamp(0.0, 10.0);
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        for (i, sub) in block.chunks_mut(REBUILD).enumerate() {
            let at = ((i + 1) * REBUILD).min(drive.len()) - 1;
            self.retune();
            let mut gain = Ramp::over(&drive[i * REBUILD..=at], boost_gain);
            for s in sub.iter_mut() {
                *s = self.step(gain.tick() * *s);
            }
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No tone control — the pedal has none. `C2` into the amp's input is
        // the only thing between the collector and the jack.
        for s in block.iter_mut() {
            let y = *s * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared() -> Rangemaster {
        let mut p = Rangemaster::new();
        p.prepare(48_000.0, OS);
        p
    }

    /// Run a sine through the transistor stage only (no Boost gain, no output
    /// network), returning the settled second half.
    fn run(p: &mut Rangemaster, amp: f32, f: f32, n: usize) -> Vec<f32> {
        let mut buf: Vec<f32> = (0..n)
            .map(|k| amp * (std::f32::consts::TAU * f * k as f32 / OS).sin())
            .collect();
        for s in buf.iter_mut() {
            *s = p.step(*s);
        }
        buf.split_off(n / 2)
    }

    fn mag_at(buf: &[f32], f: f32) -> f64 {
        let n = buf.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in buf.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(f) * i as f64 / f64::from(OS);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        2.0 * (cs * cs + cc * cc).sqrt() / n
    }

    /// **The operating point, by hand.** Everything downstream — the gain, the
    /// input impedance, which way it clips first — is a consequence of these
    /// four numbers, so they are pinned against arithmetic that shares nothing
    /// with the solver:
    ///
    /// ```text
    /// Vb ≈ V+·R1/(R1+R2)   (bias divider, lifted slightly by base current)
    /// Ve  = Vb + Veb,      Veb = Vt·ln(Ie/((1+1/βF)·Is))
    /// Ie  = (V+ − Ve)/R3
    /// Vc  = Ic·RV ≈ (βF/(βF+1))·Ie·RV
    /// ```
    #[test]
    fn the_operating_point_is_where_germanium_puts_it() {
        let p = prepared();
        let [vb, ve, vc] = p.op;

        // The divider, before base current: 7.86 V. Base current flows *out* of
        // a PNP base, which pulls the node up by a couple of hundred millivolts.
        let divider = V_PLUS * f64::from(R1) / f64::from(R1 + R2);
        assert!(
            (vb - divider).abs() < 0.35 && vb > divider,
            "Vb = {vb:.3} V against a {divider:.3} V divider"
        );

        // Germanium's forward drop, and the emitter current it implies.
        let veb = ve - vb;
        assert!(
            (0.15..0.26).contains(&veb),
            "germanium sits near 0.2 V, got {veb:.4} V"
        );
        let ie = (V_PLUS - ve) / f64::from(R3);
        assert!(
            (1.0e-4..4.0e-4).contains(&ie),
            "emitter current {ie:e} A out of range"
        );
        // Solve the same Veb from Ie the other way round and see it agree.
        let want_veb =
            f64::from(VT) * (ie / ((1.0 + 1.0 / f64::from(BETA_F)) * f64::from(IS))).ln();
        assert!(
            (veb - want_veb).abs() < 2e-3,
            "Veb {veb:.4} vs Ebers–Moll {want_veb:.4}"
        );

        // Collector: alpha·Ie through the 10 k load, and it must sit in the
        // middle of the rail or the stage has no room to swing.
        let want_vc = f64::from(BETA_F) / (f64::from(BETA_F) + 1.0) * ie * f64::from(RV);
        assert!(
            (vc - want_vc).abs() / want_vc < 0.02,
            "Vc {vc:.3} vs {want_vc:.3}"
        );
        assert!((1.0..4.0).contains(&vc), "no headroom at Vc = {vc:.3} V");
    }

    // --- the analog reference ---

    type C = (f64, f64);
    fn cmul(a: C, b: C) -> C {
        (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
    }
    fn cdiv(a: C, b: C) -> C {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    }

    /// 3×3 complex solve by Cramer's rule — small enough to be obviously right,
    /// which is the point of a reference.
    fn det3(m: [[C; 3]; 3]) -> C {
        let term = |a: C, b: C, c: C| cmul(cmul(a, b), c);
        let mut d = (0.0, 0.0);
        for (i, j, k, sign) in [
            (0usize, 1usize, 2usize, 1.0f64),
            (1, 2, 0, 1.0),
            (2, 0, 1, 1.0),
            (0, 2, 1, -1.0),
            (2, 1, 0, -1.0),
            (1, 0, 2, -1.0),
        ] {
            let t = term(m[0][i], m[1][j], m[2][k]);
            d = (d.0 + sign * t.0, d.1 + sign * t.1);
        }
        d
    }

    /// |H(jω)| of the **analog** pedal, hand-solved from the small-signal
    /// network about the operating point the test above pinned.
    ///
    /// The transistor contributes six transconductances, all of them
    /// polarity-free (flip both a voltage and a current and a conductance is
    /// unchanged), so the small-signal circuit is the same three KCL equations
    /// the solver runs, linearised — but written out here from the hybrid-π
    /// definitions rather than read from [`Bjt::stamp`]:
    ///
    /// ```text
    /// gf = Is·exp(Veb/Vt)/Vt      gr = Is·exp(Vcb/Vt)/Vt
    /// ib = (gf/βF)(vb−ve) + (gr/βR)(vb−vc)
    /// ic = gf(vb−ve) − gr(1+1/βR)(vb−vc)
    /// ie = gf(1+1/βF)(vb−ve) − gr(vb−vc)
    /// ```
    fn analog_gain(w: f64, c1: f32, op: [f64; 3]) -> f64 {
        let (vb, ve, vc) = (op[0], op[1], op[2]);
        let vt = f64::from(VT);
        let is = f64::from(IS);
        // PNP: the device's own junction voltages are Ve−Vb and Vc−Vb.
        let gf = is * ((ve - vb) / vt).exp() / vt;
        let gr = is * ((vc - vb) / vt).exp() / vt;
        let (bf, br) = (f64::from(BETA_F), f64::from(BETA_R));
        let (gib_be, gib_bc) = (gf / bf, gr / br);
        let (gic_be, gic_bc) = (gf, -gr * (1.0 + 1.0 / br));
        let (gie_be, gie_bc) = (gf * (1.0 + 1.0 / bf), -gr);

        let y1 = (0.0, w * f64::from(c1));
        let y3 = (f64::from(R3).recip(), w * f64::from(C3));
        let re = |x: f64| (x, 0.0);

        // Node equations, in the same order the solver uses. `R2` and `R3` go
        // to the rail, which is AC ground.
        let m = [
            [
                (
                    f64::from(R1).recip() + f64::from(R2).recip() + gib_be + gib_bc,
                    y1.1,
                ),
                re(-gib_be),
                re(-gib_bc),
            ],
            [re(-(gie_be + gie_bc)), (y3.0 + gie_be, y3.1), re(gie_bc)],
            [
                re(gic_be + gic_bc),
                re(-gic_be),
                re(f64::from(RV).recip() - gic_bc),
            ],
        ];
        // Right-hand side: the source drives the base node through C1 only.
        let rhs = [y1, (0.0, 0.0), (0.0, 0.0)];
        let mut mc = m;
        for r in 0..3 {
            mc[r][2] = rhs[r];
        }
        // Vc/Vs by Cramer: replace the collector column with the source term.
        let vc_over_vs = cdiv(det3(mc), det3(m));
        // det3 with a substituted column carries the sign of the cofactor
        // expansion; magnitude is all this test needs.
        vc_over_vs.0.hypot(vc_over_vs.1)
    }

    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// **The independent check on the whole circuit.** Below the point where
    /// either junction bends, the stage is linear, so its measured response must
    /// match hand-solved AC analysis of the same netlist at the operating point
    /// — across the Range knob, which moves the one component the pedal is
    /// named for.
    #[test]
    fn the_linear_response_matches_hand_solved_ac_analysis() {
        // Small enough that Vbe moves by a hundredth of a thermal voltage.
        const AMP: f32 = 2e-4;
        for range in [0.0f32, 5.0, 10.0] {
            let mut p = prepared();
            p.range = range;
            p.range_target = range;
            p.gc1 = 2.0 * f64::from(range_farads(range)) * f64::from(OS);
            p.reset();
            let c1 = range_farads(range);
            for f in [100.0f32, 400.0, 1_000.0, 3_000.0, 8_000.0] {
                let mut q = prepared();
                q.range = range;
                q.range_target = range;
                q.gc1 = p.gc1;
                q.reset();
                let y = run(&mut q, AMP, f, 1 << 16);
                let got = mag_at(&y, f) / f64::from(AMP);
                let want = analog_gain(prewarp(f), c1, q.op);
                let err = (got - want).abs() / want;
                assert!(
                    err < 0.03,
                    "range {range}, {f} Hz: model {got:.4} vs analog {want:.4} ({:.2} %)",
                    err * 100.0
                );
            }
        }
    }

    /// The signature: a high-pass input, so a low note is attenuated and a high
    /// one is boosted. This is the whole reason the pedal exists.
    #[test]
    fn it_boosts_treble_and_not_bass() {
        const AMP: f32 = 2e-4;
        let gain = |f: f32| {
            let mut p = prepared();
            let y = run(&mut p, AMP, f, 1 << 16);
            mag_at(&y, f) / f64::from(AMP)
        };
        let low = gain(100.0);
        let high = gain(4_000.0);
        assert!(
            high > 8.0 * low,
            "treble booster: 100 Hz {low:.2}× vs 4 kHz {high:.2}×"
        );
        assert!(high > 20.0, "and it must actually boost: {high:.2}×");
    }

    /// The Range knob is the input capacitor, so turning it up moves the corner
    /// down and lets bass through.
    #[test]
    fn the_range_knob_moves_the_corner() {
        const AMP: f32 = 2e-4;
        let bass_at = |range: f32| {
            let mut p = prepared();
            p.range = range;
            p.range_target = range;
            p.gc1 = 2.0 * f64::from(range_farads(range)) * f64::from(OS);
            p.reset();
            let y = run(&mut p, AMP, 200.0, 1 << 16);
            mag_at(&y, 200.0) / f64::from(AMP)
        };
        let treble = bass_at(0.0);
        let full = bass_at(10.0);
        assert!(
            full > 5.0 * treble,
            "full range must pass far more 200 Hz: {full:.2}× vs {treble:.2}×"
        );
    }

    /// **The white-box payoff.** One transistor clips asymmetrically — cutoff
    /// on one side, saturation on the other — so a symmetric sine comes out with
    /// a real second harmonic. A symmetric diode pair produces none, which is
    /// the whole difference between a booster and an overdrive.
    #[test]
    fn one_transistor_clips_asymmetrically() {
        let mut p = prepared();
        // 1.5 kHz = 192000/128: a whole number of samples per cycle, so the
        // fundamental cannot leak into the harmonic bins being measured. And it
        // is *above* the input corner — a treble booster's distortion lives
        // where its gain does, so this is where the mechanism is.
        let y = run(&mut p, 0.15, 1_500.0, 1 << 16);
        let f1 = mag_at(&y, 1_500.0);
        let f2 = mag_at(&y, 3_000.0);
        let f3 = mag_at(&y, 4_500.0);
        assert!(f1 > 0.5, "should be driven hard, fundamental {f1:.4}");
        // Measured 0.48 / 0.17. A symmetric clipper's second harmonic is zero
        // by construction, so any of this at all is the transistor.
        assert!(
            f2 > 0.25 * f1,
            "asymmetric clipping must grow a fat second harmonic: {:.3} of the fundamental",
            f2 / f1
        );
        assert!(f3 > 0.08 * f1, "and odd harmonics too: {:.3}", f3 / f1);
    }

    /// Cutoff and saturation are not the same shape, so the two half-cycles
    /// clamp at different heights — the asymmetry, seen directly.
    #[test]
    fn cutoff_and_saturation_clamp_at_different_heights() {
        let mut p = prepared();
        let y = run(&mut p, 0.6, 187.5, 1 << 15);
        let up = y.iter().fold(0.0f32, |m, s| m.max(*s));
        let down = y.iter().fold(0.0f32, |m, s| m.min(*s)).abs();
        let ratio = f64::from(up / down);
        assert!(
            !(0.8..1.25).contains(&ratio),
            "one transistor must not clip symmetrically: +{up:.3} vs −{down:.3}"
        );
    }

    /// The stage sits at a DC operating point, so silence out is the *bias*,
    /// not zero — and `step` subtracts it, which is what makes the pedal's
    /// output silent on silence.
    #[test]
    fn silence_in_silence_out() {
        let mut p = prepared();
        for k in 0..2000 {
            let y = p.step(0.0);
            assert!(y.abs() < 1e-6, "k={k}: {y}");
        }
    }

    /// RT rule 7 at the solver: slammed alternately, cold, the node voltages
    /// stay finite and inside the rails.
    #[test]
    fn bounded_when_slammed() {
        let mut p = prepared();
        for k in 0..2000 {
            let x = if k % 2 == 0 { 1.0e6 } else { -1.0e6 };
            let y = p.step(x);
            assert!(y.is_finite(), "k={k}: {y}");
            assert!(
                p.v.iter().all(|v| v.is_finite() && v.abs() < 100.0),
                "k={k}: nodes {:?}",
                p.v
            );
        }
    }
}
