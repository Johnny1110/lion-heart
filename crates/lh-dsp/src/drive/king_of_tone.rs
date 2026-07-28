//! **king-of-tone** — the Analog Man King of Tone: two solved stages in series,
//! and the family's only *soft* feedback clipper (PRD 031; Tone Revolution
//! phase 04).
//!
//! Everything else here clips one way or the other. This one does both, in
//! order: an inverting op-amp stage whose feedback carries diodes **behind a
//! resistor**, followed by a plain shunt clipper. Two roots, two solves per
//! oversampled sample.
//!
//! # Why the resistor in front of the diodes matters
//!
//! [`super::ts_wdf`] hangs its diodes directly across the feedback resistor, so
//! once they conduct they *are* the feedback and the stage's gain collapses to
//! near unity. Here the clipping branch is `R11` (6.8 kΩ) **in series** with the
//! diodes, and that whole branch sits in parallel with `R10` (220 kΩ). Conducting
//! diodes therefore pull the feedback impedance down to about 6.8 kΩ, not to
//! zero: the stage keeps roughly a thirtieth of its gain instead of losing all
//! of it.
//!
//! That is the entire "transparent overdrive" trick, and it is one resistor.
//! The knee is gradual, the stage never fully squashes, and note attacks survive
//! — which is what the Bluesbreaker family this descends from is bought for.
//! `the_series_resistor_keeps_the_gain_from_collapsing` pins it against
//! `ts-wdf`.
//!
//! # The mode switch is a diode swap
//!
//! Which is what it is on the real pedal, so it is what it is here:
//!
//! | Mode | Stage 1 feedback | Stage 2 |
//! | --- | --- | --- |
//! | **Boost** | clipping branch lifted out | bypassed |
//! | **Overdrive** | two 1N4148 in series each way | shunt pair |
//! | **Dist** | one 1N4148 each way — half the clamp, so it compresses | shunt pair |
//!
//! # Scope
//!
//! The real pedal has a linear gain stage ahead of this one (a second op-amp
//! with its own two-branch RC leg) whose pot is the Drive control. That stage is
//! not modelled; instead **`R10` is the Drive pot**, sweeping 22 kΩ to its stock
//! 220 kΩ. Same control, one stage, and the phase's non-goals allow it — but it
//! is a design choice, not a component fact, and the interaction it *doesn't*
//! reproduce is the real pot's effect on the pre-stage's bass corner.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Circuit, OnePole, knob, lp_coeff};
use crate::blocks::wdf::{
    CapacitiveVoltageSource, DiodePair, JEl, Junction, NON_INVERTING_NODES, NON_INVERTING_PORTS,
    Parallel, RType, ResistiveVoltageSource, Resistor, Series, Wdf, non_inverting_els,
};

static MODE_LABELS: [&str; 3] = ["Boost", "Overdrive", "Dist"];
static MODE_RANGE: Range = Range::Stepped {
    labels: &MODE_LABELS,
};

static PARAMS: [ParamDesc; 4] = [
    knob("drive", "Drive", 5.0, 20.0),
    ParamDesc {
        key: "mode",
        name: "Mode",
        unit: "",
        range: MODE_RANGE,
        default: 1.0,
        smoothing_ms: 0.0,
    },
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "king-of-tone",
    name: "King of Tone",
    params: &PARAMS,
};

// --- stage 1: the overdrive amplifier ---

/// Input series resistor into the inverting pin — the signal enters through the
/// gain leg, so this stage inverts.
const R9: f32 = 10e3;
/// Input coupling capacitor.
const C7: f32 = 0.1e-6;
/// Bias resistor on the non-inverting pin (grounded here — see the supply-rail
/// note shared with [`super::zendrive`]).
const R_BIAS: f32 = 1e6;
/// Stock feedback resistor, and the top of the Drive sweep.
const R10: f32 = 220e3;
/// **The resistor that makes this pedal.** In series with the clipping diodes,
/// so conducting diodes cannot take the feedback impedance below it.
const R11: f32 = 6.8e3;
/// Load on the stage output.
const RL: f32 = 1e6;

/// Op-amp constants. **Presumed part**: the schematic checked against does not
/// name it, so per ADR 033 these are same-class typicals for a modern dual
/// (3 MHz gain-bandwidth, JFET input).
const AG: f32 = 3.0e3;
const RI: f32 = 1e9;
const RO: f32 = 100.0;

// --- stage 2: the shunt clipper ---

/// Series resistance into the second stage's diodes.
const R12: f32 = 1e3;

const VT: f32 = 0.02585;
/// A 1N4148's SPICE-representative pair.
const IS: f32 = 2.52e-9;
const N_ONE: f32 = 1.75;
/// Two in series each way — the Overdrive mode's feedback pair.
const N_TWO: f32 = 2.0 * N_ONE;
/// The Boost mode lifts the clipping branch out of the loop. Modelled as a
/// device whose knee sits above anything the stage swings: with an `Is` this
/// small the root's clamp lands near 4 V, so inside the audio range the branch
/// is simply open — which is what the switch does.
const IS_OPEN: f32 = 1e-24;

static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO);

/// The family's shared amplifier junction. Note what is plugged into which
/// port: the **signal enters the gain leg**, not the input leg, which is what
/// makes this stage inverting where its cousins are not — same junction, and
/// nothing about it had to change.
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_PORTS,
};

/// Index of the load port — the stage-1 output tap.
const P_LOAD: usize = 3;

const REBUILD: usize = 64;
const TONE_HZ: f32 = 900.0;
const DC_HZ: f32 = 10.0;
/// Calibrated so the default knobs land near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.228;

/// Feedback resistance for a Drive position 0..10, audio taper. The top is the
/// stock `R10`; the bottom is where the stage is a mild, clean amplifier.
#[inline]
fn feedback_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    22_000.0 + (R10 - 22_000.0) * n * n
}

/// The gain leg, carrying the input signal into the inverting pin.
type GainLeg = Series<Resistor, CapacitiveVoltageSource>;
type OpAmpNode = RType<4, 3, (Resistor, GainLeg, Resistor)>;
/// What stage 1's diodes see: `R11` in series with them, the pair in parallel
/// with `R10` and the junction. The series resistor is the whole point.
type Stage1Tree = Series<Resistor, Parallel<OpAmpNode, Resistor>>;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Boost,
    Overdrive,
    Dist,
}

impl Mode {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Mode::Boost,
            2 => Mode::Dist,
            _ => Mode::Overdrive,
        }
    }

    /// `(Is, n)` of stage 1's feedback pair in this mode.
    fn feedback_device(self) -> (f32, f32) {
        match self {
            Mode::Boost => (IS_OPEN, N_TWO),
            Mode::Overdrive => (IS, N_TWO),
            Mode::Dist => (IS, N_ONE),
        }
    }
}

pub(super) struct KingOfTone {
    mode: Mode,
    stage1: Stage1Tree,
    d1: DiodePair,
    /// Stage 2: a source behind `R12` with a diode pair across it. The simplest
    /// WDF there is — one one-port and a root — and a reminder that the
    /// framework does not require a tree to be interesting.
    stage2: ResistiveVoltageSource,
    d2: DiodePair,
    fb_ohms: f32,
    tone_lp: OnePole,
    dc: OnePole,
    c_tone: f32,
    c_dc: f32,
}

impl KingOfTone {
    pub(super) fn new() -> Self {
        const SR0: f32 = 4.0 * 48_000.0;
        let mode = Mode::Overdrive;
        let (is, n) = mode.feedback_device();
        Self {
            mode,
            stage1: Series::new(
                Resistor::new(R11),
                Parallel::new(
                    RType::new(
                        &JUNCTION,
                        (
                            Resistor::new(R_BIAS),
                            Series::new(Resistor::new(R9), CapacitiveVoltageSource::new(C7, SR0)),
                            Resistor::new(RL),
                        ),
                    ),
                    Resistor::new(feedback_ohms(5.0)),
                ),
            ),
            d1: DiodePair::new(is, n, VT),
            stage2: ResistiveVoltageSource::new(R12),
            d2: DiodePair::new(IS, N_ONE, VT),
            fb_ohms: feedback_ohms(5.0),
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c_tone: 0.0,
            c_dc: 0.0,
        }
    }

    #[inline]
    fn set_input(&mut self, v: f32) {
        self.stage1
            .port2_mut()
            .port1_mut()
            .ports_mut()
            .1
            .port2_mut()
            .set_voltage(v);
    }

    /// Stage 1 alone: the soft feedback clipper, output taken across the load.
    #[inline]
    fn stage1_step(&mut self, x: f32) -> f32 {
        self.set_input(x);
        let a = self.stage1.reflected();
        let (_v, b) = self.d1.solve(a, self.stage1.resistance());
        self.stage1.incident(b);
        self.stage1.port2().port1().port_voltage(P_LOAD)
    }

    /// Stage 2 alone: the shunt clipper.
    #[inline]
    fn stage2_step(&mut self, y: f32) -> f32 {
        self.stage2.set_voltage(y);
        let a = self.stage2.reflected();
        let (v, b) = self.d2.solve(a, self.stage2.resistance());
        self.stage2.incident(b);
        v
    }

    /// Both stages, one oversampled sample.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        let y = self.stage1_step(x);
        if self.mode == Mode::Boost {
            return y;
        }
        self.stage2_step(y)
    }

    fn retune(&mut self, drive_pos: f32) {
        let fb = feedback_ohms(drive_pos);
        if fb != self.fb_ohms {
            self.fb_ohms = fb;
            self.stage1.port2_mut().port2_mut().set_ohms(fb);
            self.stage1.calc_impedance();
        }
    }
}

impl Circuit for KingOfTone {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_tone = lp_coeff(TONE_HZ, base_rate);
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        self.stage1.prepare(os_rate);
        self.stage1.calc_impedance();
        self.stage2.calc_impedance();
        self.reset();
    }

    fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
        self.d1.reset();
        self.d2.reset();
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn set_mode(&mut self, index: usize) {
        let mode = Mode::from_index(index);
        if mode != self.mode {
            self.mode = mode;
            let (is, n) = mode.feedback_device();
            self.d1.set_params(is, n, VT);
        }
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

    /// A tilt rather than a lowpass — this pedal is supposed to sit under what
    /// you already have, so noon is close to flat.
    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        for (s, t) in block.iter_mut().zip(tone) {
            let x = *s;
            let lp = self.tone_lp.lp(x, self.c_tone);
            let hp = x - lp;
            let n = t * 0.1;
            let bright = 0.3 + 1.6 * n * n;
            let y = (lp + bright * hp) * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared(mode: usize) -> KingOfTone {
        let mut p = KingOfTone::new();
        p.prepare(48_000.0, OS);
        p.set_mode(mode);
        p
    }

    fn run(p: &mut KingOfTone, amp: f32, f: f32, drive: f32, n: usize) -> Vec<f32> {
        let traj = vec![drive; n];
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

    fn harmonic_frac(buf: &[f32], f: f32) -> f64 {
        let fund = mag_at(buf, f);
        let total = (buf.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / buf.len() as f64)
            .sqrt()
            * std::f64::consts::SQRT_2;
        ((total * total - fund * fund).max(0.0)).sqrt() / total.max(1e-12)
    }

    /// Small-signal gain of stage 1, measured in Boost mode so the second stage
    /// is out of the way.
    fn measured_gain(f: f32, pos: f32) -> f64 {
        const AMP: f32 = 1e-4;
        let mut p = prepared(0);
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
    fn cabs(a: C) -> f64 {
        (a.0 * a.0 + a.1 * a.1).sqrt()
    }

    /// |H(jω)| of the **analog** stage 1 with its clipping branch open — an
    /// inverting amplifier, `−Zf/Zg`, with the finite-gain correction.
    ///
    /// Signal in through the gain leg means `vp = 0`, and one node equation at
    /// the inverting pin gives
    /// `vo/vin = −Ag·Zf / (Zf + Zg + Ag·Zg)`.
    fn analog_gain(w: f64, pos: f32) -> f64 {
        let zg = (f64::from(R9), -1.0 / (w * f64::from(C7)));
        let zf = (f64::from(feedback_ohms(pos)), 0.0);
        let ag = f64::from(AG);
        let num = cmul((ag, 0.0), zf);
        let den = cadd(cadd(zf, zg), cmul((ag, 0.0), zg));
        cabs(cdiv(num, den))
    }

    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// **The independent check on stage 1.** In Boost mode the clipping branch
    /// is lifted, so the stage is a linear inverting amplifier and its transfer
    /// function is textbook — hand-solved from one node equation, sharing
    /// nothing with the WDF.
    ///
    /// It also confirms the port assignment: this is the *same junction* the
    /// non-inverting pedals use, with the signal plugged into the gain leg
    /// instead of the input leg. If that swap were wrong the measured response
    /// would not be `−Zf/Zg`.
    #[test]
    fn the_linear_response_matches_hand_solved_ac_analysis() {
        for pos in [0.0f32, 5.0, 10.0] {
            for f in [100.0f32, 440.0, 2_000.0, 8_000.0] {
                let got = measured_gain(f, pos);
                let want = analog_gain(prewarp(f), pos);
                let err = (got - want).abs() / want;
                assert!(
                    err < 0.03,
                    "drive {pos}, {f} Hz: WDF {got:.4} vs analog {want:.4} ({:.2} %)",
                    err * 100.0
                );
            }
        }
    }

    /// Both roots are solved, checked against the Newton oracle.
    #[test]
    fn both_roots_track_the_newton_oracle() {
        let mut p = prepared(1);
        let mut worst = 0.0f64;
        for k in 0..40_000 {
            let t = k as f32 / OS;
            let amp = 0.002 * (1.0 + 200.0 * (k as f32 / 40_000.0));
            let x = amp * (std::f32::consts::TAU * (150.0 + 3_000.0 * t) * t).sin();

            p.set_input(x);
            let a = p.stage1.reflected();
            let r = p.stage1.resistance();
            let (v, b) = p.d1.solve(a, r);
            let (v_ref, _) = p.d1.solve_newton(a, r);
            p.stage1.incident(b);
            worst = worst.max(f64::from(v - v_ref).abs());
            let y = p.stage1.port2().port1().port_voltage(P_LOAD);

            p.stage2.set_voltage(y);
            let a2 = p.stage2.reflected();
            let r2 = p.stage2.resistance();
            let (v2, b2) = p.d2.solve(a2, r2);
            let (v2_ref, _) = p.d2.solve_newton(a2, r2);
            p.stage2.incident(b2);
            worst = worst.max(f64::from(v2 - v2_ref).abs());
            let _ = y;
        }
        assert!(
            worst < 1e-3,
            "closed form vs oracle: worst |Δv| = {worst:e} V"
        );
    }

    /// Render **stage 1 only** at a fixed drive, settled second half. The
    /// mechanism tests below live here rather than at the pedal's output
    /// because stage 2 is a hard shunt clipper: run the whole pedal and it
    /// squashes everything to the same place, hiding the very difference stage 1
    /// exists to make.
    fn stage1_run(mode: usize, amp: f32, f: f32, drive: f32, n: usize) -> Vec<f32> {
        let mut p = prepared(mode);
        p.retune(drive);
        let mut out: Vec<f32> = (0..n)
            .map(|k| {
                let x = amp * (std::f32::consts::TAU * f * k as f32 / OS).sin();
                p.stage1_step(x)
            })
            .collect();
        out.split_off(n / 2)
    }

    /// **The pedal's whole reason for existing, as a measurement.** `R11` sits
    /// in series with stage 1's diodes, so conducting diodes drop the feedback
    /// impedance to about 6.8 kΩ rather than to zero. The stage therefore keeps
    /// a usable fraction of its gain when it clips, where a Screamer — diodes
    /// straight across the feedback resistor — loses nearly all of it.
    ///
    /// Measured as compression: how much of a 12 dB input change survives to the
    /// output. More surviving means less squash.
    #[test]
    fn the_series_resistor_keeps_the_gain_from_collapsing() {
        let mine = {
            let level = |amp: f32| mag_at(&stage1_run(1, amp, 440.0, 7.0, 1 << 15), 440.0);
            level(0.05) / level(0.2)
        };
        let screamer = {
            let n = 1 << 15;
            let level = |amp: f32| {
                let mut t = super::super::ts_wdf::TsWdf::new();
                t.prepare(48_000.0, OS);
                let traj = vec![7.0f32; n];
                let mut buf: Vec<f32> = (0..n)
                    .map(|k| amp * (std::f32::consts::TAU * 440.0 * k as f32 / OS).sin())
                    .collect();
                t.shape(&mut buf, &traj);
                mag_at(&buf[n / 2..], 440.0)
            };
            level(0.05) / level(0.2)
        };
        // Fully linear would be 0.25; a hard limiter approaches 1.
        assert!(
            mine < 0.6,
            "stage 1 must stay well short of a limiter, got {mine:.3}"
        );
        assert!(
            mine < 0.8 * screamer,
            "…and preserve dynamics against ts-wdf: {mine:.3} vs {screamer:.3}"
        );
    }

    /// The mode switch is a diode swap, and each position has to be a different
    /// pedal: Boost stays clean, Overdrive breaks up, Dist breaks up harder
    /// because one diode clamps at half the height of two. Measured at stage 1,
    /// which is where the switch actually is.
    #[test]
    fn the_three_modes_are_three_pedals() {
        let dirt = |mode: usize| harmonic_frac(&stage1_run(mode, 0.1, 440.0, 7.0, 1 << 15), 440.0);
        let (boost, od, dist) = (dirt(0), dirt(1), dirt(2));
        assert!(boost < 0.02, "Boost must stay clean, got {boost:.4}");
        assert!(
            od > 5.0 * boost.max(1e-4),
            "Overdrive must break up: {od:.4} vs {boost:.4}"
        );
        assert!(
            dist > 1.3 * od,
            "Dist clamps at half the height, so it must be dirtier: {dist:.4} vs {od:.4}"
        );
    }

    /// And the modes still differ at the pedal's actual output, downstream of
    /// the second clipper — the switch has to be audible, not just measurable
    /// at an internal node.
    #[test]
    fn the_modes_still_differ_at_the_output() {
        let render = |mode: usize| {
            let mut p = prepared(mode);
            run(&mut p, 0.1, 440.0, 7.0, 1 << 14)
        };
        let outs: Vec<Vec<f32>> = (0..3).map(render).collect();
        for a in 0..3 {
            for b in (a + 1)..3 {
                let rms_a = (outs[a].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                    / outs[a].len() as f64)
                    .sqrt();
                let diff = (outs[a]
                    .iter()
                    .zip(&outs[b])
                    .map(|(x, y)| f64::from(x - y).powi(2))
                    .sum::<f64>()
                    / outs[a].len() as f64)
                    .sqrt();
                assert!(
                    diff / rms_a > 0.05,
                    "modes {a} and {b} render within {:.2} % of each other",
                    100.0 * diff / rms_a
                );
            }
        }
    }

    /// Silence in → exact silence out, in every mode.
    #[test]
    fn silence_stays_silent_in_every_mode() {
        for mode in 0..3 {
            let mut p = prepared(mode);
            for _ in 0..2_000 {
                assert_eq!(p.step(0.0), 0.0, "mode {mode} leaked");
            }
        }
    }

    /// Slammed far past anything a guitar produces, in every mode and at both
    /// ends of the pot.
    #[test]
    fn bounded_when_slammed() {
        for mode in 0..3 {
            for pos in [0.0f32, 10.0] {
                let mut p = prepared(mode);
                p.retune(pos);
                for k in 0..20_000 {
                    let y = p.step(if k % 2 == 0 { 1e6 } else { -1e6 });
                    assert!(y.is_finite(), "mode {mode}, drive {pos} went non-finite");
                    assert!(y.abs() < 1e7, "mode {mode}, drive {pos} unbounded: {y:e}");
                }
            }
        }
    }

    /// A circuit's response must not depend on how finely it is sampled.
    #[test]
    fn the_response_holds_across_sample_rates() {
        for base in [44_100.0f32, 48_000.0, 96_000.0] {
            let os = 4.0 * base;
            let mut p = KingOfTone::new();
            p.prepare(base, os);
            p.set_mode(0);
            let amp = 1e-4f32;
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
            assert!(err < 0.03, "{base} Hz: {got:.3} vs {want:.3}");
        }
    }
}
