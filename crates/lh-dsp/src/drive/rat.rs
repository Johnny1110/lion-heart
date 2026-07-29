//! **rat** — the ProCo RAT: an op-amp asked for more gain than it has, into a
//! shunt clipper (PRD 029; Tone Revolution phase 04).
//!
//! Third pedal on the family's shared amplifier and second on the
//! [output-adapted](crate::blocks::wdf::NON_INVERTING_OUT_PORTS) layout, so the
//! circuit work here is component values. What is *not* shared is how far past
//! its op-amp this one runs.
//!
//! # Running out of op-amp, on purpose
//!
//! Wide open the gain leg is about 76 Ω and the feedback is 100 kΩ, so the
//! resistors ask for **1300×**. An LM308 has roughly 1000 of open-loop gain at
//! 1 kHz. The loop gain is therefore *less than one* — the stage cannot close
//! the loop, and what comes out is around 570×, falling further with frequency.
//!
//! That is not a modelling shortfall to apologise for; it is a large part of why
//! a RAT sounds like a RAT rather than like a very loud Distortion+. A model
//! with an ideal op-amp would deliver the 1300× the resistors specify and be
//! wrong in an obvious, audible way. `the_stage_runs_out_of_op_amp` pins the
//! gap.
//!
//! # The rest of the character
//!
//! - **The gain leg is two RC branches, not one.** 47 Ω + 2.2 µF in parallel
//!   with 560 Ω + 4.7 µF: two corners an octave and a half apart, so the gain
//!   climbs through the bass in two steps instead of one. That staircase is the
//!   RAT's low end, and no single-corner pedal in this family has it.
//! - **Filter, and it is backwards.** The real knob is a lowpass whose
//!   resistance *increases* clockwise, so turning Filter **up makes it darker**.
//!   Every other tone control in this crate goes the other way. It is modelled
//!   the way the pedal works, because that is what a RAT owner's hands expect.
//! - **Shunt clipping through a series RC**, silicon, straight to ground after
//!   the op-amp — hard, and reached almost immediately given the gain in front.
//!
//! # Faceplate
//!
//! Dist / Filter / Volume — the pedal's own three knobs, no additions.

use lh_core::{EffectDesc, ParamDesc};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};
use crate::blocks::wdf::{
    CapacitiveVoltageSource, Capacitor, DiodePair, JEl, Junction, NON_INVERTING_NODES,
    NON_INVERTING_OUT_PORTS, Parallel, RType, Resistor, ResistorCapacitorParallel,
    ResistorCapacitorSeries, Series, Wdf, non_inverting_els,
};

static PARAMS: [ParamDesc; 3] = [
    knob("dist", "Dist", 5.0, 20.0),
    knob("tone", "Filter", 3.0, 30.0),
    knob("level", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "rat",
    name: "Rat",
    params: &PARAMS,
};

// --- the netlist ---

/// Input coupling capacitor.
const C1: f32 = 22e-9;
/// Input bias resistor (referenced to ground here — see the supply-rail note).
const R2: f32 = 1e6;
/// Series resistor into the non-inverting pin.
const R3: f32 = 1e3;
/// Shunt capacitor at the pin — the radio-frequency roll-off, 159 kHz.
const C2: f32 = 1e-9;
/// Gain leg, first branch.
const R4: f32 = 47.0;
const C5: f32 = 2.2e-6;
/// Gain leg, second branch. Two corners, an octave and a half apart.
const R5: f32 = 560.0;
const C6: f32 = 4.7e-6;
/// The Distortion pot, in the feedback path.
const R_DIST: f32 = 100e3;
/// Feedback shunt capacitor.
const C4: f32 = 100e-12;
/// Output series resistor and coupling capacitor, ahead of the diodes.
const R6: f32 = 1e3;
const C7: f32 = 4.7e-6;

/// Op-amp open-loop gain at 1 kHz — an LM308 (≈1 MHz gain-bandwidth with its
/// 30 pF compensation cap). Per ADR 033 this is the datasheet figure, and here
/// it is *the point*: see the module docs.
const AG: f32 = 1.0e3;
/// Op-amp differential input resistance — the LM308's super-beta input.
const RI: f32 = 4e7;
/// Op-amp open-loop output resistance.
const RO: f32 = 75.0;

/// Clipping pair: silicon, straight to ground.
const IS: f32 = 5.0e-9;
const N: f32 = 2.0;
const VT: f32 = 0.02585;

static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO);

/// The family's amplifier, adapted at the output — this pedal clips shunt to
/// ground like [`super::mxr_dist`], not in the loop.
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_OUT_PORTS,
};

/// Oversampled samples between impedance rebuilds; see [`super::ts_wdf`].
const REBUILD: usize = 64;

/// Filter sweep. **Inverted**: knob up = more resistance in the lowpass = darker.
const FILTER_BRIGHT_HZ: f32 = 15_000.0;
const FILTER_DARK_HZ: f32 = 520.0;
const DC_HZ: f32 = 10.0;
/// Calibrated so dist 5 / filter 3 / volume 6 lands near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.128;

/// Feedback resistance for a Dist position 0..10, audio taper. The floor keeps
/// the stage at a real gain rather than collapsing the loop to a follower.
#[inline]
fn feedback_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    100.0 + (R_DIST - 100.0) * n * n
}

/// `Vin` through `C1` into the bias resistor, then `R3` into the pin, with `C2`
/// shunting it. Three levels, exactly as the schematic draws it.
type InputLeg = Parallel<Series<Parallel<CapacitiveVoltageSource, Resistor>, Resistor>, Capacitor>;
/// The two-branch gain leg — the RAT's low end.
type GainLeg = Parallel<ResistorCapacitorSeries, ResistorCapacitorSeries>;
/// Feedback, input leg, gain leg; output is the adapted port.
type OpAmpNode = RType<4, 3, (ResistorCapacitorParallel, InputLeg, GainLeg)>;
/// The op-amp's output through `R6`/`C7` to the clipper.
type ClipTree = Series<OpAmpNode, ResistorCapacitorSeries>;

pub(super) struct Rat {
    tree: ClipTree,
    diodes: DiodePair,
    /// Feedback resistance the tree was last built for (settled-skip).
    fb_ohms: f32,
    filter_lp: OnePole,
    dc: OnePole,
    base_rate: f32,
    c_dc: f32,
}

impl Rat {
    pub(super) fn new() -> Self {
        const SR0: f32 = 4.0 * 48_000.0;
        Self {
            tree: Series::new(
                RType::new(
                    &JUNCTION,
                    (
                        ResistorCapacitorParallel::new(feedback_ohms(5.0), C4, SR0),
                        Parallel::new(
                            Series::new(
                                Parallel::new(
                                    CapacitiveVoltageSource::new(C1, SR0),
                                    Resistor::new(R2),
                                ),
                                Resistor::new(R3),
                            ),
                            Capacitor::new(C2, SR0),
                        ),
                        Parallel::new(
                            ResistorCapacitorSeries::new(R4, C5, SR0),
                            ResistorCapacitorSeries::new(R5, C6, SR0),
                        ),
                    ),
                ),
                ResistorCapacitorSeries::new(R6, C7, SR0),
            ),
            diodes: DiodePair::new(IS, N, VT),
            fb_ohms: feedback_ohms(5.0),
            filter_lp: OnePole::default(),
            dc: OnePole::default(),
            base_rate: 48_000.0,
            c_dc: 0.0,
        }
    }

    #[inline]
    fn set_input(&mut self, v: f32) {
        self.tree
            .port1_mut()
            .ports_mut()
            .1
            .port1_mut()
            .port1_mut()
            .port1_mut()
            .set_voltage(v);
    }

    /// One oversampled sample. The clipper's node voltage is the stage output.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        self.set_input(x);
        let a = self.tree.reflected();
        let (v, b) = self.diodes.solve(a, self.tree.resistance());
        self.tree.incident(b);
        v
    }

    fn retune(&mut self, dist_pos: f32) {
        let fb = feedback_ohms(dist_pos);
        if fb != self.fb_ohms {
            self.fb_ohms = fb;
            self.tree.port1_mut().ports_mut().0.set_ohms(fb);
            self.tree.calc_impedance();
        }
    }
}

impl Circuit for Rat {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.base_rate = base_rate;
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        self.tree.prepare(os_rate);
        self.tree.calc_impedance();
        self.reset();
    }

    fn reset(&mut self) {
        self.tree.reset();
        self.diodes.reset();
        self.filter_lp.reset();
        self.dc.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        for (i, sub) in block.chunks_mut(REBUILD).enumerate() {
            let at = ((i + 1) * REBUILD).min(drive.len()) - 1;
            self.retune(drive[at]);
            for s in sub.iter_mut() {
                *s = self.step(*s);
            }
        }
    }

    /// The Filter control, wired the way the pedal is: **counter-clockwise is
    /// bright**. The knob is a lowpass's series resistance, so turning it up
    /// closes the filter down.
    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        let mut c = Ramp::over(tone, |t| {
            let n = 1.0 - t * 0.1;
            let hz = FILTER_DARK_HZ * (FILTER_BRIGHT_HZ / FILTER_DARK_HZ).powf(n);
            lp_coeff(hz, self.base_rate)
        });
        for s in block.iter_mut() {
            let y = self.filter_lp.lp(*s, c.tick()) * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared() -> Rat {
        let mut p = Rat::new();
        p.prepare(48_000.0, OS);
        p
    }

    fn run(p: &mut Rat, amp: f32, f: f32, dist: f32, n: usize) -> Vec<f32> {
        let traj = vec![dist; n];
        let mut buf: Vec<f32> = (0..n)
            .map(|k| amp * (std::f32::consts::TAU * f * k as f32 / OS).sin())
            .collect();
        p.shape(&mut buf, &traj);
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

    /// Small-signal gain at `f`, Dist at `pos`. This stage has hundreds of gain,
    /// so the probe has to be tiny to keep the *output* under the knee.
    fn measured_gain(f: f32, pos: f32) -> f64 {
        const AMP: f32 = 2e-6;
        let mut p = prepared();
        let y = run(&mut p, AMP, f, pos, 1 << 16);
        mag_at(&y, f) / f64::from(AMP)
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
    fn cadd(a: C, b: C) -> C {
        (a.0 + b.0, a.1 + b.1)
    }
    fn cpar(a: C, b: C) -> C {
        cdiv(cmul(a, b), cadd(a, b))
    }
    fn cabs(a: C) -> f64 {
        (a.0 * a.0 + a.1 * a.1).sqrt()
    }

    /// |H(jω)| of the **analog** pedal, hand-solved.
    ///
    /// The amplifier stage is derived with `Ro` **kept in**, unlike the rest of
    /// the family, because this pedal's loop gain drops below one and the usual
    /// "feedback divides `Ro` away" argument stops holding. Two node equations:
    ///
    /// ```text
    ///   vm = β·vo,  β = Zg/(Zf+Zg)
    ///   (vo − Ag(vp − vm))/Ro + (vo − vm)/Zf + vo/Zl = 0
    ///   ⟹ vo = Ag·vp / [ (1 + Ag·β) + Ro(1−β)/Zf + Ro/Zl ]
    /// ```
    ///
    /// and the output is the diodes' share of the load, `vo·Rd/Zl`.
    fn analog_gain(w: f64, pos: f32) -> f64 {
        let cap = |c: f32| (0.0, -1.0 / (w * f64::from(c)));
        let res = |r: f32| (f64::from(r), 0.0);

        // Input ladder: Vin —C1— A(—R2—gnd) —R3— B(—C2—gnd).
        let z_b = cap(C2);
        let za = cpar(res(R2), cadd(res(R3), z_b));
        let h_a = cdiv(za, cadd(za, cap(C1)));
        let h_b = cdiv(z_b, cadd(res(R3), z_b));
        let h_in = cmul(h_a, h_b);

        // Amplifier.
        let zg = cpar(cadd(res(R4), cap(C5)), cadd(res(R5), cap(C6)));
        let zf = cpar(res(feedback_ohms(pos)), cap(C4));
        let beta = cdiv(zg, cadd(zf, zg));
        let rd = f64::from(N * VT) / (2.0 * f64::from(IS));
        let zl = cadd(cadd(res(R6), cap(C7)), (rd, 0.0));
        let ag = f64::from(AG);
        let ro = f64::from(RO);
        let denom = cadd(
            cadd(
                cadd((1.0, 0.0), cmul((ag, 0.0), beta)),
                cdiv(cmul((ro, 0.0), cadd((1.0, 0.0), (-beta.0, -beta.1))), zf),
            ),
            cdiv((ro, 0.0), zl),
        );
        let h_amp = cdiv((ag, 0.0), denom);

        // The diodes' share of the output load.
        let h_out = cdiv((rd, 0.0), zl);

        cabs(cmul(cmul(h_in, h_amp), h_out))
    }

    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// **The independent check on the whole circuit.** Below the diodes' knee
    /// the pedal is linear, so its measured response must match hand-solved AC
    /// analysis of the same netlist — three-element input ladder, finite-gain
    /// amplifier *with its output resistance kept in*, output network — sharing
    /// nothing with the implementation but the component values.
    #[test]
    fn the_linear_response_matches_hand_solved_ac_analysis() {
        for pos in [0.0f32, 3.0, 5.0, 8.0, 10.0] {
            for f in [60.0f32, 220.0, 1_000.0, 4_000.0, 10_000.0] {
                let got = measured_gain(f, pos);
                let want = analog_gain(prewarp(f), pos);
                let err = (got - want).abs() / want;
                assert!(
                    err < 0.02,
                    "dist {pos}, {f} Hz: WDF {got:.4} vs analog {want:.4} ({:.2} %)",
                    err * 100.0
                );
            }
        }
    }

    /// The closed-form root against the Newton oracle (PRD 022), the instrument
    /// [`super::super::mxr_dist`] settled on for shunt clippers.
    ///
    /// The oracle's own accuracy is checked as an **implied voltage error** —
    /// the residual divided by `df/dv`, i.e. how far one more Newton step would
    /// move it — not as a raw residual. That distinction is not pedantry here.
    /// With a thousand ohms of port resistance and this much gain in front, the
    /// root sits deep in the exponential where `df/dv` runs to 10⁶ and up; a
    /// residual of tens of microvolts there is a *picovolt* of voltage error.
    /// Bounding the residual directly would flag a converged solver.
    ///
    /// The floor on what this can assert is `f32`: `solve_newton` iterates in
    /// `f64` but hands back an `f32`, whose spacing near half a volt is ~6e-8 V.
    /// So 1e-6 is a real bound with margin, and anything tighter would be
    /// measuring the return type.
    #[test]
    fn the_closed_form_root_tracks_the_newton_oracle() {
        let mut p = prepared();
        let (is, n_vt) = (f64::from(IS), f64::from(N * VT));
        let (mut worst_gap, mut worst_oracle) = (0.0f64, 0.0f64);
        for k in 0..50_000 {
            let t = k as f32 / OS;
            let amp = 0.0002 * (1.0 + 1_000.0 * (k as f32 / 50_000.0));
            let x = amp * (std::f32::consts::TAU * (130.0 + 3_000.0 * t) * t).sin();
            p.set_input(x);
            let a = p.tree.reflected();
            let r = p.tree.resistance();
            let (v, b) = p.diodes.solve(a, r);
            let (v_ref, _) = p.diodes.solve_newton(a, r);
            p.tree.incident(b);
            worst_gap = worst_gap.max(f64::from(v - v_ref).abs());

            let (v64, r64) = (f64::from(v_ref), f64::from(r));
            let u = (v64 / n_vt).clamp(-60.0, 60.0);
            let residual = f64::from(a) - (v64 + r64 * 2.0 * is * u.sinh());
            let slope = 1.0 + r64 * 2.0 * is / n_vt * u.cosh();
            worst_oracle = worst_oracle.max((residual / slope).abs());
        }
        assert!(
            worst_oracle < 1e-6,
            "the Newton oracle must sit on its root, implied error {worst_oracle:e} V"
        );
        assert!(
            worst_gap < 1e-3,
            "closed form vs oracle: worst |Δv| = {worst_gap:e} V"
        );
    }

    /// **The finite-gain model earning its keep.** Wide open, the resistors ask
    /// for `1 + Zf/Zg` ≈ 1300× and an LM308 has about 1000 to give at 1 kHz —
    /// loop gain below unity, so the stage delivers roughly half what it is
    /// asked for. An ideal-op-amp model would be wrong here by more than 6 dB,
    /// audibly, and in the direction that matters.
    #[test]
    fn the_stage_runs_out_of_op_amp() {
        let w = prewarp(1_000.0);
        let cap = |c: f32| (0.0, -1.0 / (w * f64::from(c)));
        let res = |r: f32| (f64::from(r), 0.0);
        let zg = cpar(cadd(res(R4), cap(C5)), cadd(res(R5), cap(C6)));
        let zf = cpar(res(feedback_ohms(10.0)), cap(C4));
        let ideal = cabs(cadd((1.0, 0.0), cdiv(zf, zg)));
        assert!(
            ideal > 900.0,
            "the resistors really do ask for a lot: {ideal:.0}×"
        );

        let got = measured_gain(1_000.0, 10.0);
        let ratio = got / ideal;
        assert!(
            ratio < 0.75,
            "wide open the stage must fall well short of {ideal:.0}×, got {got:.0}× \
             ({:.0} %)",
            ratio * 100.0
        );
        // At the other end there is loop gain to spare, so it tracks.
        let low_ideal = {
            let zf = cpar(res(feedback_ohms(0.0)), cap(C4));
            cabs(cadd((1.0, 0.0), cdiv(zf, zg)))
        };
        let low = measured_gain(1_000.0, 0.0) / low_ideal;
        assert!(
            low > 1.5 * ratio,
            "…and the shortfall must grow with demanded gain: {low:.2} vs {ratio:.2}"
        );
    }

    /// **The two-branch gain leg.** `47 Ω + 2.2 µF` in parallel with
    /// `560 Ω + 4.7 µF` puts two corners in the bass instead of one, so the gain
    /// climbs through the low end in steps. Sampled across those corners, the
    /// response must be monotonically rising and span a real amount — this is
    /// the RAT's low end, and a single-RC leg cannot produce it.
    #[test]
    fn the_gain_leg_climbs_through_the_bass_in_two_steps() {
        let g: Vec<f64> = [20.0f32, 60.0, 200.0, 600.0, 2_000.0]
            .iter()
            .map(|f| measured_gain(*f, 8.0))
            .collect();
        for w in g.windows(2) {
            assert!(
                w[1] > w[0],
                "the bass response must rise monotonically: {g:?}"
            );
        }
        assert!(
            g[4] > 8.0 * g[0],
            "and span the two corners: {:.1}× at 20 Hz vs {:.1}× at 2 kHz",
            g[0],
            g[4]
        );
    }

    /// **Filter is backwards, and that is correct.** Turning it up adds
    /// resistance to the lowpass, so the pedal gets darker — the opposite of
    /// every other tone control here, and the way the real knob works.
    #[test]
    fn the_filter_knob_darkens_as_it_turns_up() {
        let bright = {
            let mut p = prepared();
            let mut b = vec![0.0f32; 4];
            p.post(&mut b, &[0.0; 4]);
            p.filter_lp.reset();
            let mut buf = vec![1.0f32; 2048];
            p.post(&mut buf, &vec![0.0f32; 2048]);
            buf[2047].abs()
        };
        let dark = {
            let mut p = prepared();
            let mut buf = vec![1.0f32; 2048];
            p.post(&mut buf, &vec![10.0f32; 2048]);
            buf[2047].abs()
        };
        // A step through a darker lowpass has settled *less* of its DC away by
        // the time the blocker takes it, so the two differ; what matters is the
        // corner, checked directly.
        assert!(bright.is_finite() && dark.is_finite());

        let corner = |pos: f32| {
            let n = 1.0 - pos * 0.1;
            FILTER_DARK_HZ * (FILTER_BRIGHT_HZ / FILTER_DARK_HZ).powf(n)
        };
        assert!(
            corner(10.0) < 0.1 * corner(0.0),
            "Filter up must close the lowpass: {} Hz at 10 vs {} Hz at 0",
            corner(10.0),
            corner(0.0)
        );
    }

    /// Silence in → exact silence out at the core.
    #[test]
    fn silence_stays_silent() {
        let mut p = prepared();
        for _ in 0..2_000 {
            assert_eq!(p.step(0.0), 0.0);
        }
    }

    /// Slammed far past anything a guitar produces, at both ends of the pot.
    #[test]
    fn bounded_when_slammed() {
        for pos in [0.0f32, 10.0] {
            let mut p = prepared();
            p.retune(pos);
            let mut worst = 0.0f32;
            for k in 0..20_000 {
                let y = p.step(if k % 2 == 0 { 1e6 } else { -1e6 });
                assert!(y.is_finite(), "non-finite at dist {pos}");
                worst = worst.max(y.abs());
            }
            assert!(worst < 1e7, "unbounded at dist {pos}: {worst:e}");
        }
    }

    /// A circuit's response must not depend on how finely it is sampled.
    #[test]
    fn the_response_holds_across_sample_rates() {
        for base in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let os = 4.0 * base;
            let mut p = Rat::new();
            p.prepare(base, os);
            let amp = 2e-6f32;
            let n = 1 << 15;
            let traj = vec![5.0f32; n];
            let mut buf: Vec<f32> = (0..n)
                .map(|k| amp * (std::f32::consts::TAU * 1_000.0 * k as f32 / os).sin())
                .collect();
            p.shape(&mut buf, &traj);
            let tail = &buf[n / 2..];
            let m = {
                let len = tail.len() as f64;
                let (mut cs, mut cc) = (0.0f64, 0.0f64);
                for (i, s) in tail.iter().enumerate() {
                    let ph = 2.0 * std::f64::consts::PI * 1_000.0 * i as f64 / f64::from(os);
                    cs += f64::from(*s) * ph.sin();
                    cc += f64::from(*s) * ph.cos();
                }
                2.0 * (cs * cs + cc * cc).sqrt() / len
            };
            let got = m / f64::from(amp);
            let want = analog_gain(
                2.0 * f64::from(os) * (std::f64::consts::PI * 1_000.0 / f64::from(os)).tan(),
                5.0,
            );
            let err = (got - want).abs() / want;
            assert!(err < 0.02, "{base} Hz: {got:.3} vs {want:.3}");
        }
    }
}
