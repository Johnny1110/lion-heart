//! **mane** — the house pedal: a drive whose tone controls act at *two
//! different places in the circuit*, and the end-to-end proof that this
//! project's framework builds pedals rather than only ports them (PRD 036;
//! Tone Revolution phase 08 §2.6).
//!
//! Every other model in this family is somebody's box. This one is not a port
//! of anything — it was designed here, against
//! `docs/tone_revolution/cookbook.md`, and every component value below is a
//! choice with a reason rather than a reading off a schematic.
//!
//! # The idea
//!
//! A drive pedal's Tone knob almost always sits *after* the clipper, so it
//! shapes what you hear but never what breaks up. That is one control doing
//! half a job, and it is why so many overdrives have one voice with a
//! brightness setting. This pedal splits the job in two:
//!
//! | knob | where it acts | what it changes |
//! | --- | --- | --- |
//! | **Focus** | the amplifier's **gain leg**, inside the feedback loop | *which frequencies get gain*, and therefore which ones reach the diodes first |
//! | **Bass / Mid / Treble** | a passive stack after the stage | what you hear, with the knob interaction and the intrinsic scoop a real one has |
//!
//! Neither is reachable with a static curve plus filters. Focus especially:
//! moving it changes the **shape of the distortion**, not the shape of the
//! spectrum — at Focus 10 a low E stays clean while the pick attack tears, and
//! at Focus 0 the whole note breaks up together.
//!
//! # Focus, physically
//!
//! The gain of a non-inverting stage is `1 + Zf/Zg`, and the gain leg `Zg` here
//! is [`R_G`] in series with a capacitor. Below that leg's corner the capacitor
//! chokes the feedback current and the gain falls to unity — clean. Above it,
//! full gain. **Focus is that capacitor**, swept two decades:
//!
//! | Focus | `C_g` | corner | what it does |
//! | --- | --- | --- | --- |
//! | 0 | 470 nF | 72 Hz | everything gets gain: fat, full-range, the whole note distorts |
//! | 5 | 47 nF | 720 Hz | the Tube Screamer's own value — a mid-hump drive |
//! | 10 | 4.7 nF | 7.2 kHz | only the top gets gain: thin and vicious over a clean body |
//!
//! Noon landing exactly on a Screamer is deliberate. It gives the knob a
//! reference point every guitarist already knows, and it makes the pedal's
//! claim checkable: at Focus 5 with the stack flat this should sit in the same
//! country as [`super::ts_wdf`], and it does
//! (`focus_at_noon_is_screamer_territory`).
//!
//! # Everything else, and why
//!
//! - **Asymmetric clipping** (2 forward, 1 reverse). Even harmonics survive a
//!   mid-scooped tone stack in a way odd ones do not — a symmetric clipper into
//!   a scoop tends to hollow out, and this pedal is *built* around having a
//!   stack after it. Same device fit as [`super::ts_wdf`]'s 1N4148 pair.
//! - **A full-range input.** [`C_IN`] into [`R_IN`] puts the coupling corner at
//!   1.6 Hz, which is a deliberate non-choice: the low end is Focus's to shape,
//!   and a second bass filter in front would fight it.
//! - **[`C_F`] across the feedback resistor**, exactly as a Screamer's `C4` does
//!   — a treble cut whose corner closes in as Drive rises (154 kHz → 6.8 kHz).
//!   Without it, Focus 10 at high drive is unlistenable rather than vicious.
//! - **A JCM800 stack**, not a Bassman: its scoop is shallower (7.4 dB against
//!   9.5 dB) and its Mid control has more range, which is what a pedal wants
//!   when the amp after it has a stack of its own.
//!
//! # Provenance
//!
//! No schematic was copied, because there is none. The op-amp constants are a
//! JFET dual's datasheet figures (ADR 033's policy); the diode fit is the one
//! [`super::ts_wdf`] already carries; the stack is Phase 02's JCM800 netlist.
//! What is new here is the arrangement.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Circuit, OnePole, ToneStack, knob, lp_coeff};
use crate::blocks::wdf::{
    AsymDiode, CapacitiveVoltageSource, JEl, Junction, NON_INVERTING_NODES, NON_INVERTING_PORTS,
    Parallel, RType, Resistor, ResistorCapacitorParallel, ResistorCapacitorSeries, Wdf,
    non_inverting_els,
};
use crate::eq::tonestack::kind;

/// Focus, as the capacitor it is. Continuous and unsmoothed at the family
/// level: it reaches a coefficient, not the sample path, so the circuit glides
/// it internally at its own rebuild rate (the [`super::Ctl::Trim`] contract).
static FOCUS_RANGE: Range = Range::Linear {
    min: 0.0,
    max: 10.0,
};

static PARAMS: [ParamDesc; 6] = [
    knob("drive", "Drive", 5.0, 20.0),
    ParamDesc {
        key: "focus",
        name: "Focus",
        unit: "",
        range: FOCUS_RANGE,
        default: 5.0,
        smoothing_ms: 0.0,
    },
    knob("bass", "Bass", 5.0, 30.0),
    knob("mid", "Mid", 5.0, 30.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "mane",
    name: "Mane",
    params: &PARAMS,
};

// --- the netlist ---

/// Input coupling capacitor. With [`R_IN`] the corner is 1.6 Hz — deliberately
/// below everything, because shaping the low end is Focus's job.
const C_IN: f32 = 100e-9;
/// Input load, `+` pin to ground. A megohm: this pedal expects a guitar.
const R_IN: f32 = 1e6;
/// Gain-leg series resistor. Fixed at the Screamer's value so that the Focus
/// sweep is a pure capacitor sweep and its corners are legible.
const R_G: f32 = 4.7e3;
/// Fixed part of the feedback resistance.
const R_F_MIN: f32 = 22e3;
/// The Drive pot, in series with [`R_F_MIN`].
const R_F_POT: f32 = 478e3;
/// Feedback shunt capacitor: the treble cut that closes in as Drive rises.
const C_F: f32 = 47e-12;
/// Load on the stage output.
const R_L: f32 = 1e6;

/// Op-amp open-loop gain — a 3 MHz gain-bandwidth JFET dual, around 1 kHz.
/// A single controlled source cannot carry the part's 6 dB/octave rolloff
/// (ADR 032: no reactive element inside an R-type netlist), so this is the
/// audio-band figure rather than the DC one; [`super::ts_wdf`]'s module docs
/// work through what that costs.
const AG: f32 = 3.0e3;
/// Op-amp differential input resistance. A JFET input is 1e12 Ω on the
/// datasheet; 1e9 is used here because the two are indistinguishable in this
/// junction — both are far larger than every other impedance in it — and the
/// smaller number keeps the scattering solve's conditioning within `f32`.
/// `tests/whitebox.rs` measures that conditioning directly.
const RI: f32 = 1e9;
/// Op-amp open-loop output resistance.
const RO: f32 = 100.0;

/// Clipping diodes: the pair-level 1N4148 fit [`super::ts_wdf`] carries.
const IS: f32 = 4.352e-9;
const N: f32 = 1.906;
const VT: f32 = 0.02585;
/// Devices in series each way — 2 forward against 1 reverse, so the two halves
/// clip at different heights and the stage makes even harmonics.
const M_FWD: f32 = 2.0;
const M_REV: f32 = 1.0;

static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO);

/// The family's shared non-inverting junction — the same one
/// [`super::ts_wdf`], [`super::zendrive`] and [`super::king_of_tone`] use, with
/// different parts hung off it. That this pedal needed no new junction, no new
/// adaptor and no new root is the phase's point: the framework was already
/// enough to design against.
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_PORTS,
};

/// Index of the load port — where the output voltage is read.
const P_LOAD: usize = 3;

const REBUILD: usize = 64;
const GLIDE_MS: f32 = 15.0;
const DC_HZ: f32 = 10.0;
/// Calibrated so the default knobs land near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.178;

/// Feedback resistance for a Drive position 0..10: 22 kΩ plus a 478 kΩ pot on a
/// square taper, so the useful range is spread over the top half of the sweep
/// the way a real pot is.
#[inline]
fn feedback_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    R_F_MIN + R_F_POT * n * n
}

/// Gain-leg capacitance for a Focus position 0..10 — two decades, geometric, so
/// equal knob movements are equal ratios and noon lands exactly on 47 nF.
#[inline]
fn focus_farads(pos: f32) -> f32 {
    470e-9 * 10f32.powf(-pos * 0.2)
}

type InputLeg = Parallel<CapacitiveVoltageSource, Resistor>;
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>;
type ClipTree = Parallel<ResistorCapacitorParallel, OpAmpNode>;

pub(super) struct Mane {
    tree: ClipTree,
    diode: AsymDiode,
    /// Feedback resistance the tree was last built for (settled-skip).
    fb_ohms: f32,
    /// Gain-leg capacitance in force, and where the Focus knob wants it.
    focus: f32,
    focus_target: f32,
    /// Per-sub-block glide coefficient toward `focus_target`.
    glide: f32,
    stack: ToneStack,
    dc: OnePole,
    c_dc: f32,
}

impl Mane {
    pub(super) fn new() -> Self {
        const SR0: f32 = 4.0 * 48_000.0;
        let focus = focus_farads(5.0);
        Self {
            tree: Parallel::new(
                ResistorCapacitorParallel::new(feedback_ohms(5.0), C_F, SR0),
                RType::new(
                    &JUNCTION,
                    (
                        Parallel::new(CapacitiveVoltageSource::new(C_IN, SR0), Resistor::new(R_IN)),
                        ResistorCapacitorSeries::new(R_G, focus, SR0),
                        Resistor::new(R_L),
                    ),
                ),
            ),
            diode: AsymDiode::new(IS, N, VT, M_FWD, M_REV),
            fb_ohms: feedback_ohms(5.0),
            focus,
            focus_target: focus,
            glide: 1.0,
            stack: ToneStack::new(kind::JCM800),
            dc: OnePole::default(),
            c_dc: 0.0,
        }
    }

    #[inline]
    fn set_input(&mut self, v: f32) {
        self.tree
            .port2_mut()
            .ports_mut()
            .0
            .port1_mut()
            .set_voltage(v);
    }

    #[inline]
    fn output(&self) -> f32 {
        self.tree.port2().port_voltage(P_LOAD)
    }

    /// One oversampled sample through the whole stage — the framework's five
    /// lines, unchanged from every other pedal built on it.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        self.set_input(x);
        let a = self.tree.reflected();
        let (_v, b) = self.diode.solve(a, self.tree.resistance());
        self.tree.incident(b);
        self.output()
    }

    /// Sub-block housekeeping: move the drive pot and glide Focus. Both change
    /// port resistances, so both need one impedance pass — and both are skipped
    /// outright when nothing moved.
    fn retune(&mut self, drive_pos: f32) {
        let mut dirty = false;

        let ohms = feedback_ohms(drive_pos);
        if ohms != self.fb_ohms {
            self.fb_ohms = ohms;
            self.tree.port1_mut().set_ohms(ohms);
            dirty = true;
        }

        // Focus is glided *geometrically* — it is a capacitance spanning two
        // decades, so a linear glide would crawl at the small end and jump at
        // the large one. Interpolating the exponent keeps the corner frequency
        // moving at a constant rate, which is what the ear tracks.
        let ratio = self.focus_target / self.focus;
        if (ratio - 1.0).abs() > 1e-4 {
            self.focus *= ratio.powf(self.glide);
            self.tree.port2_mut().ports_mut().1.set_farads(self.focus);
            dirty = true;
        }

        if dirty {
            // The capacitor states *are* the circuit's voltages, so they carry
            // across the rebuild untouched — which is what keeps a knob sweep
            // continuous instead of stepping.
            self.tree.calc_impedance();
        }
    }
}

impl Circuit for Mane {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        self.tree.prepare(os_rate);
        self.tree.calc_impedance();
        self.stack.prepare(base_rate);
        self.glide = 1.0 - (-(REBUILD as f32) / (os_rate * GLIDE_MS * 1e-3)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.tree.reset();
        self.diode.reset();
        self.stack.reset();
        self.dc.reset();
    }

    fn set_trim(&mut self, value: f32) {
        self.focus_target = focus_farads(value.clamp(0.0, 10.0));
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

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No single tone knob — tone shaping is Focus (inside the loop) and the
        // stack (in `eq`). `post` only sets the level and blocks DC, which the
        // asymmetric clipper certainly produces.
        for s in block.iter_mut() {
            let y = *s * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        self.stack.process(block, low, mid, high);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::whitebox;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared() -> Mane {
        let mut p = Mane::new();
        p.prepare(48_000.0, OS);
        p
    }

    /// The clipping stage alone, with Focus held — the device under test the
    /// whitebox kit expects (a *clipper*, not a whole pedal).
    fn stage(focus: f32, drive: f32) -> impl FnMut(f32) -> f32 {
        let mut p = prepared();
        p.set_trim(focus);
        p.focus = p.focus_target;
        p.tree.port2_mut().ports_mut().1.set_farads(p.focus);
        p.tree.port1_mut().set_ohms(feedback_ohms(drive));
        p.fb_ohms = feedback_ohms(drive);
        p.tree.calc_impedance();
        move |x| p.step(x)
    }

    /// Run the stage at fixed knobs, returning the settled second half.
    fn run(focus: f32, drive: f32, amp: f32, f: f32, n: usize) -> Vec<f32> {
        let mut dut = stage(focus, drive);
        let mut out: Vec<f32> = (0..n)
            .map(|k| dut(amp * (std::f32::consts::TAU * f * k as f32 / OS).sin()))
            .collect();
        out.split_off(n / 2)
    }

    fn mag_at(buf: &[f32], f: f32) -> f64 {
        let v: Vec<f64> = buf.iter().map(|s| f64::from(*s)).collect();
        whitebox::tone_at(&v, f64::from(OS), f64::from(f))
    }

    /// **The pedal's whole thesis.** Focus decides *which* frequencies reach the
    /// diodes, so at the same input a low note distorts hard with Focus down and
    /// stays nearly clean with Focus up — while a high note distorts either way.
    #[test]
    fn focus_chooses_which_frequencies_break_up() {
        const AMP: f32 = 0.10;
        let thd = |focus: f32, f: f32| {
            let y = run(focus, 7.0, AMP, f, 1 << 15);
            let fund = mag_at(&y, f);
            let harm: f64 = (2..8)
                .map(|m| mag_at(&y, m as f32 * f).powi(2))
                .sum::<f64>()
                .sqrt();
            harm / fund.max(1e-30)
        };

        let low_fat = thd(0.0, 93.75); // low E-ish, whole bins at OS/2048
        let low_tight = thd(10.0, 93.75);
        let high_fat = thd(0.0, 3000.0);
        let high_tight = thd(10.0, 3000.0);

        println!(
            "THD  low: focus0 {low_fat:.3} focus10 {low_tight:.3} | \
             high: focus0 {high_fat:.3} focus10 {high_tight:.3}"
        );
        assert!(
            low_tight < 0.4 * low_fat,
            "Focus up must let the lows through clean: {low_tight:.3} vs {low_fat:.3}"
        );
        assert!(
            high_tight > 0.7 * high_fat,
            "Focus up must keep distorting the highs: {high_tight:.3} vs {high_fat:.3}"
        );
    }

    /// **The hand-solved AC check** — the family's protocol for a new circuit
    /// (PRD 032 §3.1, PRD 033 §3.1): below the clipping threshold the stage is
    /// linear, so its gain has a closed form, and the model has to hit it.
    ///
    /// The closed form is the textbook non-inverting amplifier
    /// `1 + Zf/Zg`, with three corrections that all matter here:
    ///
    /// - **the clipper's zero-bias resistance.** The diode stack across the
    ///   feedback path is not an open circuit at small signal — its slope at
    ///   `v = 0` is `Is·(1/vt_f + 1/vt_r)`, i.e. 7.6 MΩ, which sits across
    ///   `Rf` and takes 1.8 % of the gain at Drive 5. Leaving it out is *the*
    ///   classic source of a "the hand calculation is a couple of percent off"
    ///   result, and it is exactly what ADR 035 §3 caught in the reference
    ///   implementation of another pedal;
    /// - **finite loop gain**: `G = G_id/(1 + G_id/Ag)`;
    /// - **the input coupling high-pass**, 1.6 Hz, which matters only at the
    ///   bottom of the sweep but is free to include.
    ///
    /// This test and the model share no reasoning: one solves a wave-digital
    /// tree sample by sample, the other evaluates a complex ratio.
    #[test]
    fn the_small_signal_gain_matches_hand_solved_ac_analysis() {
        const AMP: f32 = 1e-4; // far below the knee: the stage is linear here
        let rf = f64::from(feedback_ohms(5.0));
        let cg = f64::from(focus_farads(5.0));
        // The clipper's small-signal resistance: `di/dv` at `v = 0` inverted.
        let rd = 1.0
            / (f64::from(IS) * (1.0 / f64::from(M_FWD * N * VT) + 1.0 / f64::from(M_REV * N * VT)));
        let rf_eff = rf * rd / (rf + rd);

        let mut worst = 0.0f64;
        for f in [46.875f32, 93.75, 375.0, 750.0, 1500.0, 3000.0, 6000.0] {
            let w = std::f64::consts::TAU * f64::from(f);
            // Zf = Rf_eff ‖ 1/(jωCf); Zg = Rg − j/(ωCg)
            let zf = cdiv((rf_eff, 0.0), (1.0, w * rf_eff * f64::from(C_F)));
            let zg = (f64::from(R_G), -1.0 / (w * cg));
            let ratio = cdiv(zf, zg);
            let g_id = ((1.0 + ratio.0).hypot(ratio.1)).abs();
            let g = g_id / (1.0 + g_id / f64::from(AG));
            // Input coupling: 100 nF into 1 MΩ.
            let wr = w * f64::from(R_IN) * f64::from(C_IN);
            let want = g * wr / (1.0 + wr * wr).sqrt();

            let y = run(5.0, 5.0, AMP, f, 1 << 15);
            let got = mag_at(&y, f) / f64::from(AMP);
            let err = (got / want - 1.0).abs();
            worst = worst.max(err);
            assert!(
                err < 0.015,
                "{f} Hz: model {got:.4} vs hand analysis {want:.4} ({:.2} %)",
                100.0 * err
            );
        }
        println!("mane AC vs hand analysis: worst {:.2} %", 100.0 * worst);
    }

    /// Complex division, for the hand reference above.
    fn cdiv(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    }

    /// Focus 5 puts the gain leg on 47 nF through 4.7 kΩ, the Tube Screamer's
    /// own corner — so the *excess* gain over unity at 750 Hz must be `1/√2` of
    /// its high-frequency value, which is what "the corner is at 720 Hz" means.
    #[test]
    fn focus_at_noon_is_screamer_territory() {
        const AMP: f32 = 1e-4;
        let gain = |f: f32| {
            let y = run(5.0, 5.0, AMP, f, 1 << 15);
            mag_at(&y, f) / f64::from(AMP)
        };
        let low = gain(46.875);
        let corner = gain(750.0);
        let high = gain(6000.0);
        let frac = (corner - 1.0) / (high - 1.0);
        println!(
            "gain: 47 Hz {low:.2}, 750 Hz {corner:.2}, 6 kHz {high:.2}; corner frac {frac:.3}"
        );

        assert!(
            low < 2.5,
            "below the corner the stage must be near unity: {low:.2}"
        );
        assert!(
            high > 20.0,
            "above the corner it must reach real gain: {high:.2}"
        );
        // 6 kHz is not quite asymptotic (`C_F` has started to bite), so the
        // measured fraction sits a little above 0.707.
        assert!(
            (0.66..0.82).contains(&frac),
            "the knee must sit near 720 Hz; excess-gain fraction there is {frac:.3}"
        );
    }

    /// Sweeping Focus down moves the corner two decades, so the *low* end's
    /// gain has to climb monotonically as the knob falls. This is the test that
    /// would catch a taper written backwards.
    #[test]
    fn the_focus_sweep_is_monotone_at_the_bottom() {
        const AMP: f32 = 1e-4;
        let mut last = 0.0f64;
        for pos in [10.0, 8.0, 6.0, 4.0, 2.0, 0.0f32] {
            let y = run(pos, 5.0, AMP, 93.75, 1 << 15);
            let g = mag_at(&y, 93.75) / f64::from(AMP);
            assert!(
                g > last * 0.999,
                "focus {pos}: low-end gain {g:.3} did not rise (previous {last:.3})"
            );
            last = g;
        }
        assert!(
            last > 8.0,
            "focus 0 must gain the lows properly, got {last:.2}"
        );
    }

    /// Asymmetric by construction: two devices one way against one the other.
    /// Asymmetric by construction: two devices one way against one the other.
    /// Measured just into clipping, where the two halves' knees are furthest
    /// apart — pushed harder, both halves square off and the ratio falls back
    /// (0.43 here at THD 0.25; 0.11 at ten times the level).
    #[test]
    fn the_clipper_is_asymmetric() {
        let mut dut = stage(5.0, 7.0);
        let h = whitebox::harmonics(&mut dut, f64::from(OS), 1000.0, 0.05, 32, 8, 8);
        let ratio = h.even_over_odd();
        println!("even/odd {ratio:.3} at THD {:.3}", h.thd());
        assert!(h.thd() > 0.15, "the test premise: it must be clipping");
        assert!(
            ratio > 0.3,
            "2:1 diodes must make even harmonics, got {ratio:.3}"
        );
    }

    /// The framework's own discrimination measurements, on this circuit: it has
    /// to read as a circuit rather than as a curve, and it has to be safe.
    #[test]
    fn the_stage_is_a_circuit_and_is_well_behaved() {
        let mut dut = stage(5.0, 7.0);
        let m = whitebox::memory(&mut dut, f64::from(OS), 0.2, 16_384);
        let flat = whitebox::memory(|x| (3.0 * x).tanh(), f64::from(OS), 0.2, 16_384);
        println!("memory {m:.4} (curve floor {flat:.3e})");
        assert!(
            m > 0.05 && m > 100.0 * flat,
            "memory {m:.4} vs floor {flat:.3e}"
        );

        let mut dut = stage(5.0, 10.0);
        let peak = whitebox::bounded(&mut dut, 1e3, 4096).expect("the root diverged");
        assert!(
            peak < 1.1e3,
            "a non-inverting stage passes unity; got {peak:.1}"
        );

        let mut dut = stage(0.0, 10.0);
        assert!(
            whitebox::silent(&mut dut, 1024),
            "silence in must give exactly silence out"
        );
    }

    /// A Focus sweep under signal must not click: the state is the capacitor
    /// voltages, so a rebuild is continuous by construction — but the glide has
    /// to actually be applied, and this is what says so.
    #[test]
    fn sweeping_focus_mid_note_stays_continuous() {
        let mut p = prepared();
        let n = 1 << 14;
        let mut buf: Vec<f32> = (0..n)
            .map(|k| 0.3 * (std::f32::consts::TAU * 220.0 * k as f32 / OS).sin())
            .collect();
        let traj = vec![7.0f32; n];
        // Slam the knob from one end to the other, mid-note.
        p.set_trim(10.0);
        p.shape(&mut buf[..n / 2], &traj[..n / 2]);
        p.set_trim(0.0);
        p.shape(&mut buf[n / 2..], &traj[n / 2..]);

        let worst = buf
            .windows(2)
            .map(|w| f64::from(w[1] - w[0]).abs())
            .fold(0.0f64, f64::max);
        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("focus slam: worst step {worst:.4} on a {peak:.3} peak");
        assert!(buf.iter().all(|s| s.is_finite()));
        assert!(
            worst < 0.25 * f64::from(peak),
            "focus slam stepped by {worst:.4} on a {peak:.3} peak"
        );
    }
}
