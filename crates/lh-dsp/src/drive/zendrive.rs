//! **zendrive** — the Hermida Zendrive: the same op-amp junction as
//! [`super::ts_wdf`], different parts, and a clipper that is not a diode at all
//! (PRD 027; Tone Revolution phase 04).
//!
//! This is the pedal that shows what the phase-03 framework bought. The netlist
//! shape is *identical* to the Tube Screamer's — same nodes, same four ports,
//! same [`NON_INVERTING_PORTS`] layout — so porting it was choosing components,
//! not deriving a circuit. Neither pedal has a scattering matrix written down
//! anywhere; each builds its own from the shared topology and its own op-amp at
//! knob rate (ADR 032).
//!
//! What makes it a different pedal:
//!
//! | | Tube Screamer | Zendrive |
//! | --- | --- | --- |
//! | gain leg | fixed `4.7k + 47n` | **Voice knob**, `1k…11k + 100n` |
//! | feedback | `51k…551k ‖ 51p` | `1k…500k ‖ 100p` |
//! | input | `1µ` into `10k` | `470n` into `470k` |
//! | clipper | silicon diodes, ~0.6 V | **MOSFET stack, ~1.0 V** |
//!
//! # Why it is "transparent"
//!
//! Two reasons, both in the table. The clipper's knee sits nearly a volt higher
//! than a Screamer's, so at guitar level the stage is mostly *amplifying*, and
//! what break-up there is comes on gradually — roll the guitar volume back and
//! it walks out of clipping instead of thinning out. And the gain leg's
//! capacitor is 100 nF against the Screamer's 47 nF into a fifth of the
//! resistance, which puts the bass corner at 145 Hz–1.6 kHz instead of a fixed
//! 720 Hz: the low end comes through rather than being scooped away. There is no
//! mid-hump here. That is the entire point of the pedal.
//!
//! The **Voice** knob is that corner. Turned up it drops the gain leg to 1 kΩ —
//! more gain, and the bass corner climbs to 1.6 kHz for an upper-mid push;
//! turned down, 11 kΩ leaves the stage nearly flat and nearly clean. It sits
//! *inside* an R-type port, so moving it rebuilds the scattering matrix — at
//! sub-block rate, glided, never per sample.
//!
//! # The clipper, and what the reference model got wrong
//!
//! The Zendrive clips with two antiparallel branches, each a **1N4148 in series
//! with a diode-connected 2N7002 MOSFET**. Two devices per branch is why its
//! knee is high and its slope is shallow — and why a plain silicon `Is`/`n` pair
//! cannot describe it.
//!
//! `IS` and `THERMAL_V` here were fitted **in this project** to a SPICE sweep of
//! that clipper, over the 1 µA–300 µA the circuit actually runs at; the fit
//! tracks the curve within ±15 mV across three decades
//! (`the_clipper_matches_the_fitted_device_curve`). The reference model's
//! published pair is *not* reused: it was fitted against `Is·sinh(v/Vt)` but is
//! evaluated by `2·Is·sinh(v/nVt)`, so it clips 60–105 mV early across the whole
//! range — about the `nVt·ln 2` you would predict. See ADR 034, which also
//! corrects a guess in the phase plan: that pair is **not** a fit distorted by
//! the reference's P1/P3 wiring bug (it was fitted offline, on the standalone
//! clipper), and its ~3× thermal voltage is **not** evidence of compensation —
//! it is simply two junctions in series.
//!
//! The wiring bug itself is real and *is* corrected here: the reference gives
//! its diode pair the **input leg's** port resistance (~5 Ω) while exchanging
//! waves with the **feedback** node (~20 kΩ), a 3,600× mismatch in the `R` of
//! `a = v + R·i(v)`. Ours faces the feedback node, which is where the parts are.

use lh_core::{EffectDesc, ParamDesc};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};
use crate::blocks::wdf::{
    CapacitiveVoltageSource, DiodePair, JEl, Junction, NON_INVERTING_NODES, NON_INVERTING_PORTS,
    Parallel, RType, Resistor, ResistorCapacitorParallel, ResistorCapacitorSeries, Wdf,
    non_inverting_els,
};

static PARAMS: [ParamDesc; 4] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("voice", "Voice", 5.0, 30.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "zendrive",
    name: "Zen Drive",
    params: &PARAMS,
};

// --- the netlist ---

/// Input coupling capacitor.
const C3: f32 = 470e-9;
/// Input load on the non-inverting pin.
const R4: f32 = 470e3;
/// Fixed part of the gain leg.
const R5: f32 = 1e3;
/// The Voice pot, in series with [`R5`].
const R6: f32 = 10e3;
/// Gain-leg capacitor. With [`R5`]/[`R6`] this is the bass corner the Voice knob
/// sweeps: 145 Hz wide open, 1.6 kHz at the top.
const C5: f32 = 100e-9;
/// The Gain pot, as feedback resistance.
const R9: f32 = 500e3;
/// Feedback shunt capacitor.
const C4: f32 = 100e-12;
/// Load on the stage output.
const RL: f32 = 1e6;

/// Op-amp open-loop gain at 1 kHz. **Presumed part**: the schematic this was
/// checked against does not name the op-amp, so per ADR 033 these are same-class
/// typicals — a TL072 (3 MHz gain-bandwidth, JFET input).
const AG: f32 = 3.0e3;
/// Op-amp differential input resistance — JFET input, effectively open.
const RI: f32 = 1e9;
/// Op-amp open-loop output resistance (TL07x typical).
const RO: f32 = 200.0;

/// Saturation current of the antiparallel **1N4148 + 2N7002** branches, fitted
/// in this project to a SPICE sweep of that clipper.
const IS: f32 = 7.50e-11;
/// The pair's thermal scale in volts — `n·Vt` for the whole two-device stack,
/// not one junction's ideality, which is why it is nearly 3× a diode's.
const THERMAL_V: f32 = 0.0729;
/// Thermal voltage at room temperature, so `THERMAL_V / VT` is the effective
/// slope factor the [`DiodePair`] API wants.
const VT: f32 = 0.02585;

static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO);

/// The same junction [`super::ts_wdf`] uses — see [`NON_INVERTING_PORTS`].
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_PORTS,
};

/// Index of the load port — where the output voltage is read.
const P_LOAD: usize = 3;

/// Oversampled samples between impedance rebuilds; see [`super::ts_wdf`].
const REBUILD: usize = 64;
/// Time constant for the Voice knob's internal glide.
const GLIDE_MS: f32 = 12.0;

/// Post-stage tone: a swept treble cut, dark to open. No mid-hump, no tilt —
/// this pedal is meant to stay out of the way.
const TONE_MIN_HZ: f32 = 900.0;
const TONE_MAX_HZ: f32 = 12_000.0;
const DC_HZ: f32 = 10.0;
/// Calibrated so gain 5 / voice 5 / tone 5 / level 6 lands near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.123;

/// Feedback resistance for a gain-pot position 0..10, audio taper. The floor is
/// not cosmetic: at zero the pot would short the feedback and the stage would
/// have no gain law at all.
#[inline]
fn feedback_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    R9 * (0.002 + 0.998 * n * n)
}

/// Gain-leg resistance for a voice-pot position 0..10. **Inverted**: turning
/// Voice up takes resistance *out*, which is what raises the gain and lifts the
/// bass corner.
#[inline]
fn voice_ohms(pos: f32) -> f32 {
    R5 + R6 * (1.0 - pos * 0.1)
}

/// `Vin` through `C3`, loaded by `R4`.
type InputLeg = Parallel<CapacitiveVoltageSource, Resistor>;
/// The op-amp junction: the shared layout, this pedal's parts.
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>;
/// What the clipper sees: the feedback `(R9·gain) ‖ C4` in parallel with
/// everything the junction presents. Structurally the same type as
/// [`super::ts_wdf`]'s — the shared topology, made literal.
type ClipTree = Parallel<ResistorCapacitorParallel, OpAmpNode>;

pub(super) struct ZenDrive {
    tree: ClipTree,
    clipper: DiodePair,
    /// Component values the tree was last built for (settled-skip).
    fb_ohms: f32,
    leg_ohms: f32,
    /// Voice position in force, and where the knob wants it.
    voice: f32,
    voice_target: f32,
    glide: f32,
    tone_lp: OnePole,
    dc: OnePole,
    base_rate: f32,
    c_dc: f32,
}

impl ZenDrive {
    pub(super) fn new() -> Self {
        const SR0: f32 = 4.0 * 48_000.0;
        Self {
            tree: Parallel::new(
                ResistorCapacitorParallel::new(feedback_ohms(5.0), C4, SR0),
                RType::new(
                    &JUNCTION,
                    (
                        Parallel::new(CapacitiveVoltageSource::new(C3, SR0), Resistor::new(R4)),
                        ResistorCapacitorSeries::new(voice_ohms(5.0), C5, SR0),
                        Resistor::new(RL),
                    ),
                ),
            ),
            clipper: DiodePair::new(IS, THERMAL_V / VT, VT),
            fb_ohms: feedback_ohms(5.0),
            leg_ohms: voice_ohms(5.0),
            voice: 5.0,
            voice_target: 5.0,
            glide: 1.0,
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            base_rate: 48_000.0,
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

    /// The stage output: the voltage across the load.
    #[inline]
    fn output(&self) -> f32 {
        self.tree.port2().port_voltage(P_LOAD)
    }

    /// One oversampled sample through the whole stage.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        self.set_input(x);
        let a = self.tree.reflected();
        let (_v, b) = self.clipper.solve(a, self.tree.resistance());
        self.tree.incident(b);
        self.output()
    }

    /// Sub-block housekeeping: move the two pots, rebuild only if either
    /// actually changed. The Voice knob glides because it is a coefficient the
    /// circuit owns, not a value the sample path reads.
    fn retune(&mut self, gain_pos: f32) {
        let d = self.voice_target - self.voice;
        if d.abs() > 1e-6 {
            self.voice += d * self.glide;
        }
        let fb = feedback_ohms(gain_pos);
        let leg = voice_ohms(self.voice);
        if fb != self.fb_ohms || leg != self.leg_ohms {
            self.fb_ohms = fb;
            self.leg_ohms = leg;
            self.tree.port1_mut().set_ohms(fb);
            self.tree.port2_mut().ports_mut().1.set_ohms(leg);
            // The capacitor states are the circuit's voltages and carry across
            // untouched, which is what keeps a sweep continuous.
            self.tree.calc_impedance();
        }
    }
}

impl Circuit for ZenDrive {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.base_rate = base_rate;
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        self.tree.prepare(os_rate);
        self.tree.calc_impedance();
        self.glide = 1.0 - (-(REBUILD as f32) / (os_rate * GLIDE_MS * 1e-3)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.tree.reset();
        self.clipper.reset();
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn set_trim(&mut self, value: f32) {
        self.voice_target = value.clamp(0.0, 10.0);
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

    fn prepared() -> ZenDrive {
        let mut p = ZenDrive::new();
        p.prepare(48_000.0, OS);
        p
    }

    fn run(p: &mut ZenDrive, amp: f32, f: f32, gain: f32, n: usize) -> Vec<f32> {
        let traj = vec![gain; n];
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

    /// Small-signal gain at `f`, gain pot at `pos`, voice parked at `voice`.
    fn measured_gain(f: f32, pos: f32, voice: f32) -> f64 {
        const AMP: f32 = 1e-4;
        let mut p = prepared();
        p.voice = voice;
        p.voice_target = voice;
        let y = run(&mut p, AMP, f, pos, 1 << 16);
        mag_at(&y, f) / f64::from(AMP)
    }

    fn harmonic_frac(buf: &[f32], f: f32) -> f64 {
        let fund = mag_at(buf, f);
        let total = (buf.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / buf.len() as f64)
            .sqrt()
            * std::f64::consts::SQRT_2;
        ((total * total - fund * fund).max(0.0)).sqrt() / total.max(1e-12)
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

    /// |H(jω)| of the **analog** stage, hand-solved: input high-pass into a
    /// finite-gain non-inverting amp, `Zf = R_gain ‖ 1/(jωC4) ‖ Rd`,
    /// `Zg = R_voice + 1/(jωC5)`. `Rd` is the clipper's small-signal resistance.
    fn analog_gain(w: f64, pos: f32, voice: f32) -> f64 {
        let rf = f64::from(feedback_ohms(pos));
        let rd = f64::from(THERMAL_V) / (2.0 * f64::from(IS));
        let zf = cdiv((1.0, 0.0), (1.0 / rf + 1.0 / rd, w * f64::from(C4)));
        let zg = (f64::from(voice_ohms(voice)), -1.0 / (w * f64::from(C5)));
        let beta = cdiv(zg, cadd(zg, zf));
        let ag = f64::from(AG);
        let h_amp = cdiv((ag, 0.0), cadd((1.0, 0.0), cmul((ag, 0.0), beta)));
        let wrc = w * f64::from(R4) * f64::from(C3);
        let h_in = cdiv((0.0, wrc), (1.0, wrc));
        cabs(cmul(h_in, h_amp))
    }

    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// **The independent check on the whole circuit** — same instrument
    /// [`super::super::ts_wdf`] uses, which is the point: the two pedals share a
    /// junction, so the same hand-solved AC analysis has to describe both with
    /// nothing swapped but component values.
    ///
    /// Below the clipper's knee the stage is linear and its transfer function is
    /// textbook. Nothing here shares reasoning with the implementation — no wave
    /// variables, no scattering matrix, no tree. A mis-wired junction, a swapped
    /// port or a reversed controlled source shows up here.
    #[test]
    fn the_linear_response_matches_hand_solved_ac_analysis() {
        for voice in [0.0f32, 5.0, 10.0] {
            for pos in [0.0f32, 5.0, 10.0] {
                for f in [80.0f32, 440.0, 2_000.0, 6_000.0] {
                    let got = measured_gain(f, pos, voice);
                    let want = analog_gain(prewarp(f), pos, voice);
                    let err = (got - want).abs() / want;
                    assert!(
                        err < 0.02,
                        "voice {voice}, gain {pos}, {f} Hz: WDF {got:.4} vs analog {want:.4} \
                         ({:.2} %)",
                        err * 100.0
                    );
                }
            }
        }
    }

    /// **The clipper is the pedal.** `IS`/`THERMAL_V` were fitted to a SPICE
    /// sweep of the real thing — two antiparallel branches of a 1N4148 in series
    /// with a diode-connected 2N7002 — and this pins the fit against points read
    /// off that curve, so a future edit cannot quietly turn it back into a
    /// silicon diode pair.
    ///
    /// The points span the 1 µA–1 mA the circuit runs at. Note the shape they
    /// describe: ~0.7 V where a silicon pair is already at 0.35 V, and 165 mV
    /// per decade of current where silicon gives 115 mV. High and shallow —
    /// headroom, and a knee you can play into rather than hit.
    #[test]
    fn the_clipper_matches_the_fitted_device_curve() {
        // (current A, voltage V) from the device sweep.
        const CURVE: [(f64, f64); 4] = [
            (1.064e-6, 0.7045),
            (1.073e-5, 0.8502),
            (9.974e-5, 1.0199),
            (1.010e-3, 1.1996),
        ];
        let n_vt = f64::from(THERMAL_V);
        let is = f64::from(IS);
        for (i, v) in CURVE {
            let modelled = n_vt * (i / (2.0 * is)).asinh();
            assert!(
                (modelled - v).abs() < 0.02,
                "at {i:.3e} A the device sits at {v:.4} V, the model at {modelled:.4} V"
            );
        }
        // And it really is a higher, shallower knee than silicon (1N4148 pair,
        // as `ts-wdf` ships it) — the whole reason this pedal sounds transparent.
        let si = |i: f64| 0.04927 * (i / (2.0 * 4.352e-9)).asinh();
        let zen = |i: f64| n_vt * (i / (2.0 * is)).asinh();
        assert!(zen(1e-5) > si(1e-5) + 0.35, "the knee must sit far higher");
        let (zen_dec, si_dec) = (zen(1e-4) - zen(1e-5), si(1e-4) - si(1e-5));
        assert!(
            zen_dec > si_dec * 1.3,
            "and be shallower per decade: {zen_dec:.4} V vs silicon {si_dec:.4} V"
        );
    }

    /// The root equation is solved: `a = v + R·i(v)` for the fitted pair.
    #[test]
    fn the_clipper_root_is_solved_to_tolerance() {
        let mut p = prepared();
        let n_vt = f64::from(THERMAL_V);
        let is = f64::from(IS);
        let mut worst = 0.0f64;
        for k in 0..50_000 {
            let t = k as f32 / OS;
            let amp = 0.001 * (1.0 + 2_000.0 * (k as f32 / 50_000.0));
            let x = amp * (std::f32::consts::TAU * (120.0 + 3_000.0 * t) * t).sin();
            p.set_input(x);
            let a = p.tree.reflected();
            let r = p.tree.resistance();
            let (v, b) = p.clipper.solve(a, r);
            p.tree.incident(b);
            let i = 2.0 * is * (f64::from(v) / n_vt).sinh();
            let residual = (f64::from(a) - (f64::from(v) + f64::from(r) * i)).abs();
            worst = worst.max(residual / f64::from(a).abs().max(1e-6));
        }
        assert!(worst < 1e-3, "worst relative root residual {worst:e}");
    }

    /// **The Voice knob is the bass corner**, and it moves in the direction the
    /// faceplate promises: up takes resistance out of the gain leg, which lifts
    /// the corner and pushes the upper mids while leaving less bass through.
    #[test]
    fn voice_sweeps_the_bass_corner_and_the_gain() {
        let tilt = |v: f32| measured_gain(100.0, 5.0, v) / measured_gain(2_000.0, 5.0, v);
        let open = tilt(0.0);
        let pushed = tilt(10.0);
        assert!(
            pushed < 0.6 * open,
            "voice up must thin the bass relative to the mids: {pushed:.3} vs {open:.3}"
        );
        assert!(
            measured_gain(2_000.0, 5.0, 10.0) > 1.5 * measured_gain(2_000.0, 5.0, 0.0),
            "…and take more gain with it"
        );
    }

    /// Settled `(level, harmonic fraction)` at 440 Hz for this pedal…
    fn zen_at(amp: f32, gain: f32) -> (f64, f64) {
        let mut p = prepared();
        let y = run(&mut p, amp, 440.0, gain, 1 << 15);
        (mag_at(&y, 440.0), harmonic_frac(&y, 440.0))
    }

    /// …and for [`super::super::ts_wdf`], driven identically. The A/B is the
    /// point of this pedal existing, so it is wired into the tests rather than
    /// left to the ear alone.
    fn ts_at(amp: f32, drive: f32) -> (f64, f64) {
        let n = 1 << 15;
        let mut ts = super::super::ts_wdf::TsWdf::new();
        ts.prepare(48_000.0, OS);
        let traj = vec![drive; n];
        let mut buf: Vec<f32> = (0..n)
            .map(|k| amp * (std::f32::consts::TAU * 440.0 * k as f32 / OS).sin())
            .collect();
        ts.shape(&mut buf, &traj);
        let tail = &buf[n / 2..];
        (mag_at(tail, 440.0), harmonic_frac(tail, 440.0))
    }

    /// **The character pin.** Held against the Screamer at matched knob
    /// positions and a matched guitar-level input, this pedal distorts a third
    /// as much: its clipper's knee sits nearly a volt higher, so the stage is
    /// still amplifying where the Screamer is already squaring off.
    ///
    /// "Transparent" as a number, and measured at a *moderate* setting — which
    /// is how the pedal is used. Wound up, everything clips.
    #[test]
    fn it_stays_cleaner_than_the_screamer_at_matched_settings() {
        let (_, zen_h) = zen_at(0.05, 5.0);
        let (_, ts_h) = ts_at(0.05, 5.0);
        assert!(
            zen_h < 0.5 * ts_h,
            "zendrive must stay cleaner than ts-wdf: {zen_h:.3} vs {ts_h:.3}"
        );
    }

    /// **The other half of the character pin, and the reason people buy it: it
    /// cleans up.** Roll the guitar volume back 12 dB and the dirt should leave
    /// with it, rather than the pedal holding its own distortion up.
    ///
    /// Stated as a *comparison*, because the absolute number means little on its
    /// own — every clipper cleans up somewhat. What distinguishes this circuit is
    /// how much further it goes than a Screamer over the same gesture, and that
    /// falls out of the high, shallow MOSFET knee: the signal walks back out
    /// through it instead of sitting on top of it.
    #[test]
    fn it_cleans_up_far_better_than_the_screamer() {
        let (zen_loud, zen_loud_h) = zen_at(0.2, 5.0);
        let (zen_soft, zen_soft_h) = zen_at(0.05, 5.0);
        let (_, ts_loud_h) = ts_at(0.2, 5.0);
        let (_, ts_soft_h) = ts_at(0.05, 5.0);

        let zen_ratio = zen_soft_h / zen_loud_h;
        let ts_ratio = ts_soft_h / ts_loud_h;
        assert!(
            zen_soft_h < 0.4 * zen_loud_h,
            "backing off must clean it up: {zen_soft_h:.3} at −12 dB vs {zen_loud_h:.3}"
        );
        assert!(
            zen_ratio < 0.5 * ts_ratio,
            "…and far more than the Screamer does: {zen_ratio:.2} vs {ts_ratio:.2} \
             of the distortion retained"
        );
        assert!(zen_soft < zen_loud, "and the level follows the input down");
    }

    /// Dynamics, the same claim read off the *level* instead of the distortion:
    /// at a moderate gain the Zendrive's output tracks its input nearly
    /// linearly, where the Screamer's is already compressed into its clipper.
    #[test]
    fn its_level_tracks_the_input_where_the_screamer_compresses() {
        let zen = zen_at(0.05, 3.0).0 / zen_at(0.2, 3.0).0;
        let ts = ts_at(0.05, 3.0).0 / ts_at(0.2, 3.0).0;
        // A perfectly linear stage would give 0.25 over this 12 dB step.
        assert!(
            zen < 0.7 * ts,
            "zendrive should track its input far more closely: {zen:.3} vs \
             screamer {ts:.3} (linear would be 0.25)"
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

    /// Slammed far past anything a guitar produces, at both ends of both pots.
    #[test]
    fn bounded_when_slammed() {
        for gain in [0.0f32, 10.0] {
            for voice in [0.0f32, 10.0] {
                let mut p = prepared();
                p.voice = voice;
                p.voice_target = voice;
                let mut worst = 0.0f32;
                for k in 0..20_000 {
                    let y = p.step(if k % 2 == 0 { 1e6 } else { -1e6 });
                    assert!(y.is_finite(), "non-finite at gain {gain}, voice {voice}");
                    worst = worst.max(y.abs());
                }
                assert!(worst < 1e7, "unbounded at gain {gain}: {worst:e}");
            }
        }
    }

    /// A circuit's response must not depend on how finely it is sampled.
    #[test]
    fn the_response_holds_across_sample_rates() {
        for base in [44_100.0f32, 48_000.0, 96_000.0] {
            let os = 4.0 * base;
            let mut p = ZenDrive::new();
            p.prepare(base, os);
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
                5.0,
            );
            let err = (got - want).abs() / want;
            assert!(err < 0.02, "{base} Hz: {got:.3} vs {want:.3}");
        }
    }

    /// The Voice knob must glide, not step: it moves a port resistance, and a
    /// step in that is a step in both gain and frequency response.
    #[test]
    fn the_voice_knob_glides_instead_of_stepping() {
        let mut p = prepared();
        p.set_trim(10.0);
        let traj = vec![5.0f32; 4096];
        let mut buf = vec![0.0f32; 4096];
        let before = p.voice;
        p.shape(&mut buf, &traj);
        assert!(before < p.voice, "the glide must move");
        assert!(p.voice < 10.0, "…and not arrive in one block ({})", p.voice);
        for _ in 0..24 {
            p.shape(&mut buf, &traj);
        }
        assert!(
            (p.voice - 10.0).abs() < 1e-2,
            "settles at target, {}",
            p.voice
        );
    }
}
