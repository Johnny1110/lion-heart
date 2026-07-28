//! **diode-clipper** — one diode, wired four ways (PRD 030; Tone Revolution
//! phase 04, the platform piece).
//!
//! Every other pedal in this family is a specific box. This one is the
//! *framework*, with knobs on it: pick a topology, pick a device, pick how many
//! of them, and hear what each choice does with everything else held still.
//!
//! It exists for two reasons. It is the honest A/B rig — comparing `ts-wdf` to
//! `mxr-dist` conflates the clipper's position with a dozen component values,
//! whereas here only the wiring changes. And it is the worked example for
//! anyone (including the author) building a circuit on
//! [`crate::blocks::wdf`]: four trees, two roots, and the observation that
//! **root and tree compose independently** — the Rectify mode is the Shunt tree
//! with an asymmetric root, no other change.
//!
//! # The four topologies
//!
//! | Circuit | Tree | Root | What it does |
//! | --- | --- | --- | --- |
//! | **Shunt** | source ‖ cap | symmetric pair | the classic: clips to a fixed knee, and the cap makes the knee move with frequency |
//! | **Series** | source in series with a load | symmetric pair | a *dead zone* — small signals cannot get through the diodes at all, so it is the one that gets cleaner as you play softer, not dirtier |
//! | **Rectify** | source ‖ cap | asymmetric, 2 forward / 1 reverse | one half clips lower than the other, so it makes even harmonics: the "tube-like" trick |
//! | **Feedback** | the family's op-amp junction | symmetric pair | the diodes fight an amplifier instead of a resistor, which is why op-amp overdrives sound soft |
//!
//! The component values are deliberately plain — a legible resistance, a round
//! capacitor — and are **not** claimed to be any real pedal. This is the one
//! model here where that is the point.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};
use crate::blocks::wdf::{
    AsymDiode, CapacitiveVoltageSource, Capacitor, DiodePair, JEl, Junction, NON_INVERTING_NODES,
    NON_INVERTING_PORTS, Parallel, RType, ResistiveVoltageSource, Resistor,
    ResistorCapacitorParallel, ResistorCapacitorSeries, Series, Wdf, non_inverting_els,
};

static CIRCUIT_LABELS: [&str; 4] = ["Shunt", "Series", "Rectify", "Feedback"];
static CIRCUIT_RANGE: Range = Range::Stepped {
    labels: &CIRCUIT_LABELS,
};

/// Selectable devices, as `(Is, n)` — the convention ADR 033 settled on, since a
/// knee needs both numbers and germanium is not silicon with a different `Is`.
/// Plain SPICE representatives here rather than any pedal's fitted pair.
static DIODE_LABELS: [&str; 3] = ["Si", "Ge", "LED"];
static DIODE_MODEL: [(f32, f32); 3] = [(2.52e-9, 1.75), (2.0e-7, 1.28), (1.0e-16, 2.0)];
static DIODE_RANGE: Range = Range::Stepped {
    labels: &DIODE_LABELS,
};

/// Series devices per branch. Continuous, because the thermal scale it moves is.
static COUNT_RANGE: Range = Range::Linear { min: 0.3, max: 3.0 };

static PARAMS: [ParamDesc; 6] = [
    knob("drive", "Drive", 5.0, 20.0),
    ParamDesc {
        key: "circuit",
        name: "Circuit",
        unit: "",
        range: CIRCUIT_RANGE,
        default: 0.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "diode",
        name: "Diode",
        unit: "",
        range: DIODE_RANGE,
        default: 0.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "count",
        name: "Count",
        unit: "",
        range: COUNT_RANGE,
        default: 1.0,
        smoothing_ms: 0.0,
    },
    knob("tone", "Tone", 6.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "diode-clipper",
    name: "Diode Lab",
    params: &PARAMS,
};

// --- component values: plain on purpose ---

/// Series resistance from the driving stage into the clipping node.
const R_SERIES: f32 = 2200.0;
/// Shunt capacitor across the diodes — what makes the Shunt mode's knee move
/// with frequency. With `R_SERIES` the corner is 3.3 kHz.
const C_SHUNT: f32 = 22e-9;
/// Load the Series mode drives through its diodes.
const R_LOAD: f32 = 10e3;

// The Feedback mode's op-amp stage: a textbook non-inverting overdrive.
const FB_R: f32 = 100e3;
const FB_C: f32 = 100e-12;
const LEG_R: f32 = 4.7e3;
const LEG_C: f32 = 47e-9;
const IN_C: f32 = 1e-6;
const IN_R: f32 = 470e3;
const LOAD_R: f32 = 1e6;
/// A garden-variety modern dual op-amp: 3 MHz gain-bandwidth, JFET input.
const AG: f32 = 3.0e3;
const RI: f32 = 1e9;
const RO: f32 = 100.0;

const VT: f32 = 0.02585;

static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO);
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_PORTS,
};

const REBUILD: usize = 64;
const GLIDE_MS: f32 = 10.0;
const TONE_MIN_HZ: f32 = 700.0;
const TONE_MAX_HZ: f32 = 14_000.0;
const DC_HZ: f32 = 10.0;
/// Calibrated so the default knobs land near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.157;

/// Pre-clipper gain for a Drive position 0..10 — the same law the Screamers use,
/// so "drive 6" means the same thing here as it does over there.
#[inline]
fn drive_gain(pos: f32) -> f32 {
    let n = pos * 0.1;
    1.0 + (51_000.0 + 500_000.0 * n * n) / 4_700.0
}

type ShuntTree = Parallel<ResistiveVoltageSource, Capacitor>;
type SeriesTree = Series<ResistiveVoltageSource, Resistor>;
type InputLeg = Parallel<CapacitiveVoltageSource, Resistor>;
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>;
type FeedbackTree = Parallel<ResistorCapacitorParallel, OpAmpNode>;

/// Which of the four wirings is live.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Shunt,
    SeriesPath,
    Rectify,
    Feedback,
}

impl Mode {
    fn from_index(i: usize) -> Self {
        match i {
            1 => Mode::SeriesPath,
            2 => Mode::Rectify,
            3 => Mode::Feedback,
            _ => Mode::Shunt,
        }
    }
}

pub(super) struct DiodeClipper {
    mode: Mode,
    /// All four trees exist at once: switching is an enum write, never an
    /// allocation, and each keeps its own state so a switch is a crossfade
    /// between two settled circuits rather than a transient.
    shunt: ShuntTree,
    series: SeriesTree,
    feedback: FeedbackTree,
    /// Two roots. Rectify is the Shunt *tree* with this one instead of `pair` —
    /// the cleanest demonstration that root and tree are independent.
    pair: DiodePair,
    asym: AsymDiode,
    is: f32,
    n: f32,
    count: f32,
    count_target: f32,
    glide: f32,
    /// Feedback resistance the tree was last built for (settled-skip).
    fb_ohms: f32,
    tone_lp: OnePole,
    dc: OnePole,
    base_rate: f32,
    c_dc: f32,
}

impl DiodeClipper {
    pub(super) fn new() -> Self {
        const SR0: f32 = 4.0 * 48_000.0;
        let (is, n) = DIODE_MODEL[0];
        Self {
            mode: Mode::Shunt,
            shunt: Parallel::new(
                ResistiveVoltageSource::new(R_SERIES),
                Capacitor::new(C_SHUNT, SR0),
            ),
            series: Series::new(ResistiveVoltageSource::new(R_SERIES), Resistor::new(R_LOAD)),
            feedback: Parallel::new(
                ResistorCapacitorParallel::new(FB_R, FB_C, SR0),
                RType::new(
                    &JUNCTION,
                    (
                        Parallel::new(CapacitiveVoltageSource::new(IN_C, SR0), Resistor::new(IN_R)),
                        ResistorCapacitorSeries::new(LEG_R, LEG_C, SR0),
                        Resistor::new(LOAD_R),
                    ),
                ),
            ),
            pair: DiodePair::new(is, n, VT),
            asym: AsymDiode::new(is, n, VT, 2.0, 1.0),
            is,
            n,
            count: 1.0,
            count_target: 1.0,
            glide: 1.0,
            fb_ohms: FB_R,
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            base_rate: 48_000.0,
            c_dc: 0.0,
        }
    }

    fn refresh_devices(&mut self) {
        let nvt = self.count * self.n;
        self.pair.set_params(self.is, nvt, VT);
        self.asym.set_params(self.is, nvt, VT);
    }

    /// One oversampled sample through whichever wiring is selected.
    ///
    /// Four branches, but the shape is identical every time: drive the tree's
    /// source, gather the reflected wave, solve the root, push the reflection
    /// back down, read a voltage. That sameness is the framework's whole claim.
    #[inline]
    fn step(&mut self, e: f32) -> f32 {
        match self.mode {
            Mode::Shunt => {
                self.shunt.port1_mut().set_voltage(e);
                let a = self.shunt.reflected();
                let (v, b) = self.pair.solve(a, self.shunt.resistance());
                self.shunt.incident(b);
                v
            }
            Mode::Rectify => {
                self.shunt.port1_mut().set_voltage(e);
                let a = self.shunt.reflected();
                let (v, b) = self.asym.solve(a, self.shunt.resistance());
                self.shunt.incident(b);
                v
            }
            Mode::SeriesPath => {
                self.series.port1_mut().set_voltage(e);
                let a = self.series.reflected();
                let r = self.series.resistance();
                let (v, b) = self.pair.solve(a, r);
                self.series.incident(b);
                // The diodes are *in* the path, so the output is what the loop
                // current develops across the load. Read it from the port
                // current, `i = (a − v)/R`, rather than by subtracting the drop
                // from the source: the series adaptor presents `−e` at the root,
                // so a hand-written `e − v` silently *adds* the drop instead of
                // removing it — and then the dead zone disappears, which is the
                // whole reason this mode exists.
                (v - a) * (R_LOAD / (R_SERIES + R_LOAD))
            }
            Mode::Feedback => {
                self.feedback
                    .port2_mut()
                    .ports_mut()
                    .0
                    .port1_mut()
                    .set_voltage(e);
                let a = self.feedback.reflected();
                let (_v, b) = self.pair.solve(a, self.feedback.resistance());
                self.feedback.incident(b);
                self.feedback.port2().port_voltage(3)
            }
        }
    }

    fn retune(&mut self, drive_pos: f32) {
        let d = self.count_target - self.count;
        if d.abs() > 1e-6 {
            self.count += d * self.glide;
            self.refresh_devices();
        }
        // Only the Feedback wiring has a knob inside its tree; the others take
        // the Drive knob as a plain pre-gain.
        if self.mode == Mode::Feedback {
            let n = drive_pos * 0.1;
            let fb = 51_000.0 + (FB_R * 5.0 - 51_000.0) * n * n;
            if fb != self.fb_ohms {
                self.fb_ohms = fb;
                self.feedback.port1_mut().set_ohms(fb);
                self.feedback.calc_impedance();
            }
        }
    }
}

impl Circuit for DiodeClipper {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.base_rate = base_rate;
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        self.shunt.prepare(os_rate);
        self.shunt.calc_impedance();
        self.series.prepare(os_rate);
        self.series.calc_impedance();
        self.feedback.prepare(os_rate);
        self.feedback.calc_impedance();
        self.glide = 1.0 - (-(REBUILD as f32) / (os_rate * GLIDE_MS * 1e-3)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.shunt.reset();
        self.series.reset();
        self.feedback.reset();
        self.pair.reset();
        self.asym.reset();
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn set_mode(&mut self, index: usize) {
        let mode = Mode::from_index(index);
        if mode != self.mode {
            self.mode = mode;
            // The outgoing tree keeps its state; the incoming one is settled
            // already because every tree runs its own reset at prepare and none
            // of them accumulate while unselected.
            self.tone_lp.reset();
        }
    }

    fn set_shape(&mut self, index: usize) {
        let (is, n) = DIODE_MODEL[index.min(DIODE_MODEL.len() - 1)];
        if is != self.is || n != self.n {
            self.is = is;
            self.n = n;
            self.refresh_devices();
        }
    }

    fn set_trim(&mut self, value: f32) {
        self.count_target = value.clamp(0.3, 3.0);
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        for (i, sub) in block.chunks_mut(REBUILD).enumerate() {
            let at = ((i + 1) * REBUILD).min(drive.len()) - 1;
            self.retune(drive[at]);
            if self.mode == Mode::Feedback {
                for s in sub.iter_mut() {
                    *s = self.step(*s);
                }
            } else {
                let g = drive_gain(drive[at]);
                for s in sub.iter_mut() {
                    *s = self.step(g * *s);
                }
            }
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        let mut c = Ramp::over(tone, |t| {
            let hz = TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(t * 0.1);
            lp_coeff(hz, self.base_rate)
        });
        for s in block.iter_mut() {
            let y = self.tone_lp.lp(*s, c.tick()) * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared(mode: usize) -> DiodeClipper {
        let mut p = DiodeClipper::new();
        p.prepare(48_000.0, OS);
        p.set_mode(mode);
        p
    }

    fn run(p: &mut DiodeClipper, amp: f32, f: f32, drive: f32, n: usize) -> Vec<f32> {
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

    /// The faceplate and the mode table have to agree, or the Circuit knob picks
    /// something other than what it says.
    #[test]
    fn the_circuit_menu_is_aligned() {
        assert_eq!(CIRCUIT_RANGE.max(), 3.0);
        assert_eq!(PARAMS[1].range.max(), CIRCUIT_RANGE.max());
        assert_eq!(PARAMS[2].range.max(), (DIODE_MODEL.len() - 1) as f32);
        for (i, want) in [
            (0, Mode::Shunt),
            (1, Mode::SeriesPath),
            (2, Mode::Rectify),
            (3, Mode::Feedback),
        ] {
            assert!(
                Mode::from_index(i) == want,
                "index {i} picks the wrong wiring"
            );
        }
    }

    /// **Every wiring must be a different circuit**, not four labels over one
    /// sound. Checked pairwise on the rendered audio at a setting where all four
    /// are working.
    #[test]
    fn all_four_wirings_sound_different() {
        let render = |mode: usize| {
            let mut p = prepared(mode);
            run(&mut p, 0.2, 440.0, 6.0, 1 << 14)
        };
        let outs: Vec<Vec<f32>> = (0..4).map(render).collect();
        for a in 0..4 {
            let rms_a = (outs[a].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                / outs[a].len() as f64)
                .sqrt();
            assert!(rms_a > 1e-4, "wiring {a} produced nothing");
            for b in (a + 1)..4 {
                let diff = outs[a]
                    .iter()
                    .zip(&outs[b])
                    .map(|(x, y)| f64::from(x - y).powi(2))
                    .sum::<f64>()
                    / outs[a].len() as f64;
                let rel = diff.sqrt() / rms_a;
                assert!(
                    rel > 0.05,
                    "wirings {a} and {b} render within {:.2} % of each other",
                    rel * 100.0
                );
            }
        }
    }

    /// **Rectify is the Shunt tree with an asymmetric root** — the pedal's
    /// teaching point stated as a measurement. Same wiring, same devices, and
    /// the only difference is that one root clips its two halves at different
    /// heights, which is exactly what puts even harmonics in.
    #[test]
    fn rectify_adds_even_harmonics_that_shunt_does_not() {
        // 187.5 Hz is 192 kHz / 1024, so a power-of-two window holds a whole
        // number of cycles and the fundamental does not leak into the harmonic
        // bin. At 220 Hz it does, by more than the effect being measured.
        const F0: f32 = 187.5;
        let second_harmonic = |mode: usize| {
            let mut p = prepared(mode);
            let y = run(&mut p, 0.3, F0, 7.0, 1 << 15);
            mag_at(&y, 2.0 * F0) / mag_at(&y, F0).max(1e-12)
        };
        let shunt = second_harmonic(0);
        let rectify = second_harmonic(2);
        assert!(
            shunt < 0.01,
            "a matched pair must suppress the 2nd harmonic, got {shunt:.4}"
        );
        assert!(
            rectify > 10.0 * shunt.max(1e-5),
            "…and 2-vs-1 diodes must create it: {rectify:.4} vs {shunt:.4}"
        );
    }

    /// **Series is the one that behaves backwards.** With the diodes *in* the
    /// path, a small signal cannot open them at all, so quiet playing comes out
    /// cleaner-but-quieter rather than dirtier — the dead-zone character no
    /// shunt clipper can produce. Measured as: its output level falls off faster
    /// than proportionally as the input drops, which is the opposite of every
    /// clipping wiring.
    #[test]
    fn the_series_wiring_has_a_dead_zone() {
        let level = |mode: usize, amp: f32| {
            let mut p = prepared(mode);
            mag_at(&run(&mut p, amp, 375.0, 3.0, 1 << 14), 375.0)
        };
        // Twenty times less input, i.e. 26 dB — proportional would be 0.05.
        let series_ratio = level(1, 0.01) / level(1, 0.2);
        let shunt_ratio = level(0, 0.01) / level(0, 0.2);
        assert!(
            series_ratio < 0.05,
            "series must be expansive — below proportional — got {series_ratio:.4}"
        );
        assert!(
            shunt_ratio > 3.0 * series_ratio,
            "…where shunt compresses instead: {shunt_ratio:.4} vs {series_ratio:.4}"
        );
    }

    /// The Diode knob moves the knee in the right direction, and the Count knob
    /// stacks it — the same two controls as [`super::super::ts_wdf`], here with
    /// nothing else in the way to confound them.
    #[test]
    fn the_device_controls_move_the_knee() {
        let peak = |diode: usize, count: f32| {
            let mut p = prepared(0);
            p.set_shape(diode);
            p.set_trim(count);
            // Let the count glide land before measuring.
            let mut warm = vec![0.0f32; 1 << 14];
            p.shape(&mut warm, &vec![5.0f32; 1 << 14]);
            let y = run(&mut p, 0.3, 440.0, 6.0, 1 << 14);
            y.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let si = peak(0, 1.0);
        let ge = peak(1, 1.0);
        let led = peak(2, 1.0);
        assert!(ge < 0.8 * si, "germanium clamps lower: {ge:.3} vs {si:.3}");
        assert!(led > 1.5 * si, "an LED clamps higher: {led:.3} vs {si:.3}");
        assert!(
            peak(0, 3.0) > 1.5 * peak(0, 0.5),
            "stacking devices must raise the clamp"
        );
    }

    /// The Shunt wiring's cap is not decoration: it diverts current away from
    /// the diodes as frequency rises, so a high note breaks up **less** than a
    /// low one at the same drive. That frequency dependence is what a memoryless
    /// curve cannot do, and it is the reason this mode is the default.
    ///
    /// The Series wiring has no such cap, so it is the control: its break-up
    /// must stay far flatter across the same two octaves.
    #[test]
    fn the_shunt_cap_makes_break_up_frequency_dependent() {
        let frac = |mode: usize, f: f32| {
            let mut p = prepared(mode);
            harmonic_frac(&run(&mut p, 0.15, f, 7.0, 1 << 15), f)
        };
        let (shunt_low, shunt_high) = (frac(0, 187.5), frac(0, 6_000.0));
        assert!(
            shunt_low > 1.2 * shunt_high,
            "highs must break up less than lows: 187 Hz {shunt_low:.3} vs 6 kHz {shunt_high:.3}"
        );
        let (series_low, series_high) = (frac(1, 187.5), frac(1, 6_000.0));
        let shunt_tilt = shunt_high / shunt_low;
        let series_tilt = series_high / series_low;
        assert!(
            series_tilt > shunt_tilt,
            "the capless wiring must be flatter across frequency: \
             {series_tilt:.3} vs {shunt_tilt:.3}"
        );
    }

    /// Silence in → exact silence out, in every wiring.
    #[test]
    fn silence_stays_silent_in_every_wiring() {
        for mode in 0..4 {
            let mut p = prepared(mode);
            for _ in 0..2_000 {
                assert_eq!(p.step(0.0), 0.0, "wiring {mode} leaked");
            }
        }
    }

    /// Slammed far past anything a guitar produces, in every wiring and on
    /// every device — including the LED, whose saturation current is twelve
    /// orders below silicon's and is the stiffest thing here.
    #[test]
    fn bounded_when_slammed_in_every_wiring() {
        for mode in 0..4 {
            for diode in 0..3 {
                let mut p = prepared(mode);
                p.set_shape(diode);
                for k in 0..10_000 {
                    let y = p.step(if k % 2 == 0 { 1e6 } else { -1e6 });
                    assert!(
                        y.is_finite(),
                        "wiring {mode}, device {diode} went non-finite"
                    );
                    assert!(
                        y.abs() < 1e7,
                        "wiring {mode}, device {diode} unbounded: {y:e}"
                    );
                }
            }
        }
    }

    /// Switching wiring mid-note must not produce a spike: every tree is settled
    /// and holds its own state, so the worst case is a step, never a blow-up.
    #[test]
    fn switching_wiring_mid_note_stays_bounded() {
        let mut p = prepared(0);
        let n = 1 << 13;
        let mut buf: Vec<f32> = (0..n)
            .map(|k| 0.3 * (std::f32::consts::TAU * 220.0 * k as f32 / OS).sin())
            .collect();
        let traj = vec![6.0f32; n];
        for (i, sub) in buf.chunks_mut(512).enumerate() {
            p.set_mode(i % 4);
            let t = &traj[..sub.len()];
            p.shape(sub, t);
        }
        for s in &buf {
            assert!(s.is_finite() && s.abs() < 50.0, "switch produced {s}");
        }
    }
}
