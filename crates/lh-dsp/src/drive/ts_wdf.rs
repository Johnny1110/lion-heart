//! **ts-wdf** — the Tube Screamer's clipping amplifier, whole, as one solved
//! circuit: the op-amp, its feedback network, the diodes *inside* that network,
//! the gain leg, the input coupling and the load, all one Wave Digital Filter
//! solved every oversampled sample (PRD 026; Tone Revolution phase 04).
//!
//! This is the fourth and most complete member of a deliberate set. Same pedal,
//! four models, so the difference each layer of physics makes is audible rather
//! than argued:
//!
//! | pedal | clipper | op-amp |
//! | --- | --- | --- |
//! | [`super::ts9`] | memoryless curve | a gain number |
//! | [`super::screamer`] | WDF shunt diodes + cap | a gain number |
//! | [`super::sd1`] | WDF diodes in the loop | *ideal* (virtual short) |
//! | **`ts-wdf`** | WDF diodes in the loop | **finite gain, real input and output impedance** |
//!
//! # Why the topology matters
//!
//! In the real TS the diodes are not across the signal — they are across the
//! **feedback path** of a non-inverting amplifier. That changes the character
//! at the root:
//!
//! - The stage's gain is `1 + Zf/Zg`, and `Zg` is `R4` in series with `C3`. Below
//!   that leg's corner (~720 Hz) `C3` chokes the feedback current, the gain falls
//!   to unity, and bass walks through *clean*. The famous mid-hump is not a
//!   filter someone added around a clipper; it is what this topology does.
//!   [`super::screamer`] has to build it by hand (high-pass the gained path, sum
//!   the dry) — here it emerges.
//! - `C4` (51 pF) across the feedback resistor is a one-pole treble cut whose
//!   corner *moves with the drive pot*: 61 kHz wide open at drive 0, 5.7 kHz at
//!   drive 10. Turning up genuinely darkens the pedal.
//! - Because the diodes see the feedback node rather than the signal node, they
//!   clip against a frequency-dependent impedance. The clipping threshold is not
//!   one number.
//!
//! # The op-amp, and a deliberate departure from the reference
//!
//! The op-amp is a **finite-gain** device folded into the junction's scattering
//! matrix: open-loop gain [`AG`], input resistance [`RI`], output resistance
//! [`RO`]. Those three are model parameters, not circuit components, so they are
//! *not* facts to be inherited — and the reference implementation's trio
//! (`Ag = 100`, `Ri = 1 GΩ`, `Ro = 0.1 Ω`) is not a JRC4558. `Ag = 100` is
//! roughly where a 4558's open-loop gain has fallen by **30 kHz**; across the
//! guitar band the part actually offers 3000 down to ~400.
//!
//! Using 100 measurably suppresses two of this pedal's signatures — it flattens
//! the top of the drive sweep (drive 10 asks for 117× and gets 54×) and it
//! muffles the `C4` treble cut the phase's own acceptance criteria call for. So
//! the numbers here are the datasheet's, and the deviation is on the record.
//!
//! One constant still cannot be right everywhere: a real op-amp's gain falls at
//! 6 dB/octave, and modelling that pole needs a reactive element *inside* the
//! junction, which an R-type netlist does not carry (ADR 032). The top octave is
//! therefore modelled with more loop gain than the part has. What the model does
//! deliver is that the shortfall from the textbook `1 + Zf/Zg` is computed
//! rather than assumed, and it grows with demanded gain exactly as loop gain
//! predicts (`the_shortfall_from_ideal_grows_with_demanded_gain`) — which is the
//! machinery the lower-loop-gain pedals later in this family will lean on.
//!
//! # Faceplate
//!
//! Drive / Diode / Count / Tone / Level. The two middle knobs are the mod
//! everybody does to a Screamer: swap the clipping diodes, or stack more of
//! them. `Count` is continuous rather than integer because what it scales — the
//! pair's thermal voltage `m·n·Vt` — is continuous; 1.5 is not a diode and a
//! half, it is a knee halfway between one and two.
//!
//! The tone stage is deliberately *identical* to [`super::screamer`]'s, so an
//! A/B between the two is a comparison of clipping models and nothing else.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Circuit, OnePole, knob, lp_coeff};
use crate::blocks::wdf::{
    CapacitiveVoltageSource, DiodePair, JEl, Junction, Parallel, RType, Resistor,
    ResistorCapacitorParallel, ResistorCapacitorSeries, Wdf, op_amp,
};

/// Clipping diodes offered by the Diode knob.
///
/// A diode's knee needs **both** numbers: `Is` sets where it starts conducting,
/// `n` (ideality) sets how fast the current climbs after that, and
/// `v ≈ n·Vt·ln(i/Is)` mixes them. Germanium is *not* "silicon with a different
/// `Is`" — it is high `Is` **and** near-unity `n`, and that pairing is what puts
/// its knee at ~0.3 V instead of ~0.6 V.
///
/// A note on the reference model, because it is a trap worth recording: BYOD's
/// diode menu carries `Is` alone and folds ideality into its user-facing
/// "# Diodes" knob, with `1N34 → 200 pA`. Held against a silicon `n`, that value
/// makes the *germanium* setting clip **higher** than the silicon one — backwards
/// for the part it names, and 1000× off the 200 nA the circulated 1N34A SPICE
/// model gives. We carry `(Is, n)` per device and use `IS=2.0e-7, N=1.28` for the
/// 1N34A, so the Diode knob moves the knee the way the parts do.
///
/// - `1N4148` — the stock pair, and a **pair-level** fit (4.352 nA, n 1.906)
///   rather than single-device SPICE: it absorbs the real pair's mismatch and
///   bulk resistance, and it is what the TS was checked against. Default.
/// - `GZ34` — plain small-signal silicon (the figures [`super::screamer`] uses).
/// - `1N34` — germanium, ~half the clamp: softer, earlier, quieter break-up.
/// - `LED` — the other mod everybody does. Roughly 1.5 V at 1 mA; an
///   order-of-magnitude fit for a red LED, not a datasheet extraction. Loud,
///   with far more headroom before it squares off.
static DIODE_LABELS: [&str; 4] = ["1N4148", "GZ34", "1N34", "LED"];
/// `(Is, n)` per entry of [`DIODE_LABELS`], same order.
static DIODE_MODEL: [(f32, f32); 4] = [
    (4.352e-9, 1.906),
    (2.52e-9, 1.75),
    (2.0e-7, 1.28),
    (1.0e-16, 2.0),
];
static DIODE_RANGE: Range = Range::Stepped {
    labels: &DIODE_LABELS,
};

/// Series diodes per branch — a multiplier on the selected device's `n·Vt`.
/// Continuous, because the thermal scale is: see [`DiodePair::set_params`].
static COUNT_RANGE: Range = Range::Linear { min: 0.3, max: 3.0 };
/// One diode per branch — stock.
const COUNT_DEFAULT: f32 = 1.0;

static PARAMS: [ParamDesc; 5] = [
    knob("drive", "Drive", 5.0, 20.0),
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
        default: COUNT_DEFAULT,
        // Glided inside the circuit at sub-block rate instead (`GLIDE_MS`) —
        // the family's smoothers feed the per-sample path, and this value only
        // reaches a coefficient.
        smoothing_ms: 0.0,
    },
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "ts-wdf",
    name: "TS WDF",
    params: &PARAMS,
};

// --- the netlist: TS-808/TS-9 clipping amplifier (IC1B) ---

/// `+` input to ground.
const R5: f32 = 10e3;
/// Input coupling capacitor.
const C2: f32 = 1e-6;
/// Gain-leg series resistor (`−` input to ground).
const R4: f32 = 4.7e3;
/// Gain-leg series capacitor — the mid-hump's corner, `1/(2π·R4·C3)` ≈ 720 Hz.
const C3: f32 = 0.047e-6;
/// Fixed part of the feedback resistance.
const R6: f32 = 51e3;
/// The drive pot, in series with [`R6`].
const POT1: f32 = 500e3;
/// Feedback shunt capacitor — the treble cut that closes in as drive rises.
const C4: f32 = 51e-12;
/// Load on the stage output.
const RL: f32 = 1e6;

/// Op-amp open-loop gain — a JRC4558 around 1 kHz (≈3 MHz gain-bandwidth).
/// See the module docs for why this is a compromise and what it costs.
const AG: f32 = 3.0e3;
/// Op-amp differential input resistance (JRC4558 typical).
const RI: f32 = 5e6;
/// Op-amp open-loop output resistance (JRC4558 typical). Feedback divides it by
/// the loop gain, so the stage drives its load from well under an ohm.
const RO: f32 = 75.0;

/// Thermal voltage at room temperature.
const VT: f32 = 0.02585;

// Junction nodes. 0 is ground.
const N_PLUS: u8 = 1;
const N_MINUS: u8 = 2;
const N_OUT: u8 = 3;
const N_INTERNAL: u8 = 4;

static OPAMP: [JEl; 3] = op_amp(N_PLUS, N_MINUS, N_OUT, N_INTERNAL, AG, RI, RO);

/// The op-amp junction. Series/parallel reduction cannot express it — the
/// controlled source ties the output back to the input pair — so it is an
/// R-type adaptor whose scattering matrix is built from *this* netlist at knob
/// rate (ADR 032), never transcribed.
///
/// The up port is the **feedback path**, output to inverting input. That is
/// where the diodes hang, and it is a high-impedance point (roughly
/// `(AG+1)·Zg` ≈ 480 kΩ at audio), so the adaptation `R_up = R_thévenin` is
/// well conditioned — unlike the op-amp's own output pin, which feedback drives
/// to milliohms (ADR 032 §5).
static JUNCTION: Junction = Junction {
    nodes: 5,
    els: &OPAMP,
    ports: &[
        (N_OUT, N_MINUS), // 0 — up: feedback network + diodes
        (N_PLUS, 0),      // 1 — input leg: C2 (carrying Vin) ‖ R5
        (N_MINUS, 0),     // 2 — gain leg: R4 + C3
        (N_OUT, 0),       // 3 — load: RL, and the stage's output tap
    ],
};

/// Index of the load port — where the output voltage is read.
const P_LOAD: usize = 3;

/// Oversampled samples between impedance rebuilds. At 192 kHz that is a 3 kHz
/// rebuild rate, far above any knob gesture, and a settled knob costs one
/// float compare (the `eq::chain` / `eq::tonestack` convention).
const REBUILD: usize = 64;
/// Time constant for the Count knob's internal glide.
const GLIDE_MS: f32 = 10.0;

/// Post-stage tone tilt corner — *identical* to [`super::screamer`]'s, on
/// purpose: with the same tone stage on both, an A/B between them is a
/// comparison of clipping models and nothing else.
const TONE_HZ: f32 = 723.0;
const DC_HZ: f32 = 10.0;
/// Calibrated so drive 5 / tone 5 / level 6 lands near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.30;

/// Feedback resistance for a drive-pot position 0..10 — 51 kΩ plus the 500 kΩ
/// pot on an audio taper. The same law [`super::ts9`] and [`super::screamer`]
/// use, so "drive 6" means the same thing on all four Screamers.
#[inline]
fn feedback_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    R6 + POT1 * n * n
}

/// `Vin` through `C2`, loaded by `R5` — the stage's input port.
type InputLeg = Parallel<CapacitiveVoltageSource, Resistor>;
/// The op-amp and its wiring: three ports plus the adapted feedback port.
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>;
/// The whole stage as the diode root sees it: the feedback `(R6+pot) ‖ C4` in
/// parallel with everything the op-amp junction presents.
type ClipTree = Parallel<ResistorCapacitorParallel, OpAmpNode>;

pub(super) struct TsWdf {
    tree: ClipTree,
    diode: DiodePair,
    /// Feedback resistance the tree was last built for (settled-skip).
    fb_ohms: f32,
    /// Selected device's `(Is, n)`.
    is: f32,
    n: f32,
    /// Diode count in force, and where the knob wants it.
    count: f32,
    count_target: f32,
    /// Per-sub-block glide coefficient toward `count_target`.
    glide: f32,
    tone_lp: OnePole,
    dc: OnePole,
    c_tone: f32,
    c_dc: f32,
}

impl TsWdf {
    pub(super) fn new() -> Self {
        // Rebuilt for the real rate in `prepare`; the tree only needs *a* rate
        // to exist at.
        const SR0: f32 = 4.0 * 48_000.0;
        Self {
            tree: Parallel::new(
                ResistorCapacitorParallel::new(feedback_ohms(5.0), C4, SR0),
                RType::new(
                    &JUNCTION,
                    (
                        Parallel::new(CapacitiveVoltageSource::new(C2, SR0), Resistor::new(R5)),
                        ResistorCapacitorSeries::new(R4, C3, SR0),
                        Resistor::new(RL),
                    ),
                ),
            ),
            diode: DiodePair::new(DIODE_MODEL[0].0, COUNT_DEFAULT * DIODE_MODEL[0].1, VT),
            fb_ohms: feedback_ohms(5.0),
            is: DIODE_MODEL[0].0,
            n: DIODE_MODEL[0].1,
            count: COUNT_DEFAULT,
            count_target: COUNT_DEFAULT,
            glide: 1.0,
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c_tone: 0.0,
            c_dc: 0.0,
        }
    }

    /// Drive the input capacitor's source — the one place audio enters.
    #[inline]
    fn set_input(&mut self, v: f32) {
        self.tree
            .port2_mut()
            .ports_mut()
            .0
            .port1_mut()
            .set_voltage(v);
    }

    /// The stage output: the voltage across the load, i.e. the op-amp's output
    /// pin. Free — both of that port's waves are already in the adaptor.
    #[inline]
    fn output(&self) -> f32 {
        self.tree.port2().port_voltage(P_LOAD)
    }

    /// One oversampled sample through the whole stage.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        self.set_input(x);
        let a = self.tree.reflected();
        let (_v, b) = self.diode.solve(a, self.tree.resistance());
        self.tree.incident(b);
        self.output()
    }

    /// Sub-block housekeeping: move the drive pot and glide the diode count.
    /// Both are coefficient updates — never per sample, and skipped entirely
    /// when nothing moved.
    fn retune(&mut self, drive_pos: f32) {
        let ohms = feedback_ohms(drive_pos);
        if ohms != self.fb_ohms {
            self.fb_ohms = ohms;
            self.tree.port1_mut().set_ohms(ohms);
            // The capacitor states *are* the circuit's voltages, so they carry
            // across the rebuild untouched — which is what keeps a knob sweep
            // continuous instead of stepping.
            self.tree.calc_impedance();
        }
        let d = self.count_target - self.count;
        if d.abs() > 1e-6 {
            self.count += d * self.glide;
            self.diode.set_params(self.is, self.count * self.n, VT);
        }
    }
}

impl Circuit for TsWdf {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_tone = lp_coeff(TONE_HZ, base_rate);
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        // Every reactive element is discretized at the rate its solver runs at,
        // so port resistances — and with them the scattering matrix — have to
        // be rebuilt from the root down.
        self.tree.prepare(os_rate);
        self.tree.calc_impedance();
        self.glide = 1.0 - (-(REBUILD as f32) / (os_rate * GLIDE_MS * 1e-3)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.tree.reset();
        self.diode.reset();
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn set_shape(&mut self, index: usize) {
        let (is, n) = DIODE_MODEL[index.min(DIODE_MODEL.len() - 1)];
        if is != self.is || n != self.n {
            self.is = is;
            self.n = n;
            // Stepped, so it steps: swapping a diode is a setup gesture, and
            // gliding `Is` across three decades would be a fiction.
            self.diode.set_params(is, self.count * n, VT);
        }
    }

    fn set_trim(&mut self, value: f32) {
        self.count_target = value.clamp(0.3, 3.0);
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
        for (s, t) in block.iter_mut().zip(tone) {
            let x = *s;
            let lp = self.tone_lp.lp(x, self.c_tone);
            let hp = x - lp;
            let n = t * 0.1;
            // 0 = dark (8% of the treble), 10 = bright (+3.4 dB tilt).
            let bright = 0.08 + 1.4 * n * n;
            let y = (lp + bright * hp) * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: f32 = 4.0 * 48_000.0;

    fn prepared() -> TsWdf {
        let mut p = TsWdf::new();
        p.prepare(48_000.0, OS);
        p
    }

    /// Run the stage at one drive position, returning the settled second half.
    fn run(p: &mut TsWdf, amp: f32, f: f32, drive: f32, n: usize) -> Vec<f32> {
        let traj = vec![drive; n];
        let mut buf: Vec<f32> = (0..n)
            .map(|k| amp * (std::f32::consts::TAU * f * k as f32 / OS).sin())
            .collect();
        p.shape(&mut buf, &traj);
        buf.split_off(n / 2)
    }

    /// Magnitude of `buf` at `f` (one Goertzel bin, in the same units as the
    /// input amplitude).
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

    /// Small-signal gain the stage actually delivers at `f`, drive `pos`.
    ///
    /// The amplitude has to leave the diodes in their linear region at the
    /// *output*, not the input: at drive 10 the stage gives ~85×, and 85 mV
    /// across a pair whose thermal scale is 49 mV is already 50 % into the
    /// `sinh`. A tenth of a millivolt in keeps every setting honestly linear.
    fn measured_gain(f: f32, pos: f32) -> f64 {
        const AMP: f32 = 1e-4;
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
    fn cabs(a: C) -> f64 {
        (a.0 * a.0 + a.1 * a.1).sqrt()
    }

    /// |H(jω)| of the **analog** clipping amplifier, solved on paper rather than
    /// by the WDF: input high-pass × a finite-gain non-inverting amp.
    ///
    /// `Zf = (R6+pot) ‖ 1/(jωC4) ‖ Rd`, `Zg = R4 + 1/(jωC3)`,
    /// `β = Zg/(Zg+Zf)`, `H_amp = Ag/(1 + Ag·β)`.
    ///
    /// `Rd = nVt/(2·Is)` is the diodes' small-signal resistance — they are never
    /// truly open, and at 5.7 MΩ they shave a couple of percent off `Zf`.
    /// `Ri`/`Ro` are left ideal; at 1 GΩ and 0.1 Ω they move this by ~1e-7.
    fn analog_gain(w: f64, pos: f32) -> f64 {
        let wrc = w * f64::from(R5) * f64::from(C2);
        let h_in = cdiv((0.0, wrc), (1.0, wrc));
        cabs(cmul(h_in, (amp_gain(w, pos, f64::from(AG)), 0.0)))
    }

    /// Just the amplifier's `Vout/V+`, at open-loop gain `ag`. Passing `ag =
    /// ∞` (well, `1e12`) gives the textbook `1 + Zf/Zg` an ideal op-amp would
    /// deliver, which is how the finite-gain shortfall gets measured.
    fn amp_gain(w: f64, pos: f32, ag: f64) -> f64 {
        let rf = f64::from(feedback_ohms(pos));
        let (is, n) = DIODE_MODEL[0];
        let rd = f64::from(COUNT_DEFAULT * n * VT) / (2.0 * f64::from(is));
        let zf = cdiv((1.0, 0.0), (1.0 / rf + 1.0 / rd, w * f64::from(C4)));
        let zg = (f64::from(R4), -1.0 / (w * f64::from(C3)));
        let beta = cdiv(zg, cadd(zg, zf));
        cabs(cdiv((ag, 0.0), cadd((1.0, 0.0), cmul((ag, 0.0), beta))))
    }

    /// Bilinear pre-warp: a Tustin-discretized network's response at digital
    /// frequency `f` is the analog response at this ω, not at `2πf`.
    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// **The independent check on the whole circuit.** Below the diodes' knee
    /// the stage is linear, and its transfer function is a textbook one — so the
    /// WDF's measured response must match hand-solved AC analysis of the same
    /// netlist, across the band and across the drive pot.
    ///
    /// Nothing here shares reasoning with the implementation: no wave variables,
    /// no scattering matrix, no adaptor algebra, no tree. If the junction were
    /// mis-wired, a port swapped, the op-amp's controlled source stamped
    /// backwards, or the up port attached to the wrong node, this is where it
    /// would show — and it covers the R-type construction end to end in the one
    /// regime where an exact answer exists.
    #[test]
    fn the_linear_response_matches_hand_solved_ac_analysis() {
        for pos in [0.0f32, 5.0, 10.0] {
            for f in [50.0f32, 220.0, 1_000.0, 3_000.0, 8_000.0] {
                let got = measured_gain(f, pos);
                let want = analog_gain(prewarp(f), pos);
                let err = (got - want).abs() / want;
                assert!(
                    err < 0.02,
                    "drive {pos}, {f} Hz: WDF gain {got:.4} vs analog {want:.4} ({:.2} %)",
                    err * 100.0
                );
            }
        }
    }

    /// The root equation really is solved: `a = v + R·i(v)` with
    /// `i = 2·Is·sinh(v/nVt)`, at every amplitude from below the knee to a
    /// slammed input.
    #[test]
    fn the_diode_root_is_solved_to_tolerance() {
        let mut p = prepared();
        let n_vt = f64::from(p.count * p.n * VT);
        let is = f64::from(p.is);
        let mut worst = 0.0f64;
        for k in 0..50_000 {
            let t = k as f32 / OS;
            let amp = 0.001 * (1.0 + 2_000.0 * (k as f32 / 50_000.0));
            let x = amp * (std::f32::consts::TAU * (150.0 + 4_000.0 * t) * t).sin();
            p.set_input(x);
            let a = p.tree.reflected();
            let r = p.tree.resistance();
            let (v, b) = p.diode.solve(a, r);
            p.tree.incident(b);
            let i = 2.0 * is * (f64::from(v) / n_vt).sinh();
            let residual = (f64::from(a) - (f64::from(v) + f64::from(r) * i)).abs();
            worst = worst.max(residual / f64::from(a).abs().max(1e-6));
        }
        assert!(worst < 1e-3, "worst relative root residual {worst:e}");
    }

    /// **The signature.** The mid-hump is not a filter bolted onto a clipper —
    /// it is `C3` in the gain leg choking the feedback current below ~720 Hz, so
    /// the stage's own gain falls toward unity in the bass while the mids get
    /// the full ratio. Measured in the linear regime, where it is purely the
    /// topology talking.
    #[test]
    fn the_mid_hump_comes_from_the_gain_leg() {
        let low = measured_gain(100.0, 5.0);
        let mid = measured_gain(1_000.0, 5.0);
        assert!(
            mid > 3.0 * low,
            "mids should tower over lows: 100 Hz {low:.2}× vs 1 kHz {mid:.2}×"
        );
        // And the bass is not merely quieter — it is close to unclipped unity,
        // which is why a TS keeps its low end tight instead of woofy.
        assert!(low < 8.0, "bass gain should stay modest, got {low:.2}×");
    }

    /// **The second signature.** `C4` sits across the feedback resistor, so its
    /// corner is `1/(2π·Rf·C4)` and `Rf` *is* the drive pot: 61 kHz at drive 0,
    /// 5.7 kHz at drive 10. Turning up darkens the pedal — the complaint every
    /// Screamer owner has, falling out of two components.
    #[test]
    fn turning_up_the_drive_darkens_the_stage() {
        let tilt = |pos: f32| measured_gain(8_000.0, pos) / measured_gain(1_000.0, pos);
        let open = tilt(1.0);
        let cranked = tilt(10.0);
        assert!(
            cranked < 0.7 * open,
            "drive 10 must roll off the top vs drive 1: {cranked:.3} vs {open:.3}"
        );
    }

    /// **The white-box payoff, and the two-sided one no memoryless shaper
    /// reproduces.** This stage's break-up is band-limited *from both ends by
    /// two different components*: below ~720 Hz `C3` starves the gain leg, above
    /// the moving `C4` corner the feedback shunt does — so at one fixed input
    /// level the mids square off while the bass and the top octave stay
    /// comparatively clean.
    ///
    /// A curve applied to a filtered signal can fake one side. Faking both, with
    /// the corners moving as the drive pot turns, means building this circuit.
    #[test]
    fn break_up_is_band_limited_from_both_ends() {
        let frac = |f: f32| {
            let mut p = prepared();
            let y = run(&mut p, 0.02, f, 9.0, 1 << 15);
            let fund = mag_at(&y, f);
            let total = (y.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / y.len() as f64)
                .sqrt()
                * std::f64::consts::SQRT_2;
            ((total * total - fund * fund).max(0.0)).sqrt() / total
        };
        let bass = frac(120.0);
        let mid = frac(1_000.0);
        let top = frac(12_000.0);
        assert!(
            mid > bass * 1.2,
            "the gain leg must keep the bass cleaner than the mids: \
             120 Hz {bass:.3} vs 1 kHz {mid:.3}"
        );
        assert!(
            mid > top * 1.2,
            "C4 must keep the top octave cleaner than the mids: \
             12 kHz {top:.3} vs 1 kHz {mid:.3}"
        );
    }

    /// The finite-gain op-amp is doing arithmetic, not decoration: the stage
    /// always lands *under* the textbook `1 + Zf/Zg`, and the gap widens as the
    /// drive pot demands more gain — because the shortfall is `1/(1 + Ag·β)` and
    /// `β` is what the pot moves.
    ///
    /// Pinning the *trend* rather than one number is the point. It is what a
    /// future edit that quietly idealises the op-amp away would break, and it is
    /// the mechanism the low-loop-gain pedals later in this family depend on.
    #[test]
    fn the_shortfall_from_ideal_grows_with_demanded_gain() {
        let w = prewarp(1_000.0);
        // The input high-pass is common to both, so it cancels out of the ratio
        // and the comparison is purely about the amplifier.
        let h_in = {
            let wrc = w * f64::from(R5) * f64::from(C2);
            cabs(cdiv((0.0, wrc), (1.0, wrc)))
        };
        let shortfall = |pos: f32| {
            let ideal = amp_gain(w, pos, 1e12) * h_in;
            // Measured, not merely modelled: it is the WDF that has to fall
            // short, not just the analysis of it.
            let got = measured_gain(1_000.0, pos);
            assert!(
                got < ideal,
                "drive {pos}: {got:.2}× must sit under {ideal:.2}×"
            );
            (ideal - got) / ideal
        };
        let low = shortfall(0.0);
        let high = shortfall(10.0);
        assert!(
            high > 3.0 * low,
            "the shortfall must grow with demanded gain: \
             drive 0 {:.2} % vs drive 10 {:.2} %",
            low * 100.0,
            high * 100.0
        );
    }

    /// The Diode knob is a real parts swap, ordered the way the parts are:
    /// germanium clamps well below silicon, an LED well above it. Getting this
    /// ordering right is exactly what carrying `(Is, n)` per device buys — with
    /// `Is` alone the germanium entry lands on the *wrong side* of silicon (see
    /// [`DIODE_MODEL`]), which is the bug this test exists to keep out.
    #[test]
    fn the_diode_selector_moves_the_knee_the_right_way() {
        let peak_for = |index: usize| {
            let mut p = prepared();
            p.set_shape(index);
            let y = run(&mut p, 0.2, 500.0, 7.0, 1 << 14);
            y.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let silicon = peak_for(0);
        let germanium = peak_for(2);
        let led = peak_for(3);
        assert!(
            germanium < 0.75 * silicon,
            "germanium must clamp lower: {germanium:.3} V vs silicon {silicon:.3} V"
        );
        assert!(
            led > 1.5 * silicon,
            "an LED must clamp higher: {led:.3} V vs silicon {silicon:.3} V"
        );
    }

    /// The menu and the device table are two arrays that have to agree.
    #[test]
    fn the_diode_menu_is_aligned() {
        assert_eq!(DIODE_LABELS.len(), DIODE_MODEL.len());
        assert_eq!(DIODE_RANGE.max(), (DIODE_MODEL.len() - 1) as f32);
        // The faceplate's stepped param is the menu.
        assert_eq!(PARAMS[1].range.max(), DIODE_RANGE.max());
    }

    /// The Count knob stacks diodes, and stacking raises the clamp. It glides
    /// rather than steps, so the check runs long enough for the glide to land.
    #[test]
    fn stacking_diodes_raises_the_clamp() {
        let peak_for = |count: f32| {
            let mut p = prepared();
            p.set_trim(count);
            let y = run(&mut p, 0.2, 500.0, 7.0, 1 << 14);
            y.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let one = peak_for(0.5);
        let three = peak_for(3.0);
        assert!(
            three > 1.5 * one,
            "three diodes should clamp far higher than half of one: \
             {three:.3} V vs {one:.3} V"
        );
    }

    /// The Count knob must not step: a jump in `n·Vt` is a jump in the clipping
    /// threshold, and at these levels that is a click. The glide is inside the
    /// circuit because the value never reaches the per-sample path — only a
    /// coefficient — so the family's smoothers would be the wrong tool.
    #[test]
    fn the_count_knob_glides_instead_of_stepping() {
        let mut p = prepared();
        p.set_trim(3.0);
        let traj = vec![7.0f32; 4096];
        let mut buf = vec![0.0f32; 4096];
        // Silence in: `count` still has to walk, and we watch it walk.
        let before = p.count;
        p.shape(&mut buf, &traj);
        assert!(before < p.count, "the glide must move");
        assert!(
            p.count < 3.0,
            "…and must not arrive inside one 4096-sample block ({})",
            p.count
        );
        // It does arrive, though.
        for _ in 0..16 {
            p.shape(&mut buf, &traj);
        }
        assert!(
            (p.count - 3.0).abs() < 1e-3,
            "settles at target, {}",
            p.count
        );
    }

    /// Silence in → exact silence out at the core, and nothing sustains: every
    /// reactive state decays to zero and the solver's fixed point at `a = 0` is
    /// `v = 0`.
    #[test]
    fn silence_stays_silent() {
        let mut p = prepared();
        for _ in 0..2_000 {
            assert_eq!(p.step(0.0), 0.0);
        }
    }

    /// Slammed four orders past anything a guitar produces, at both ends of the
    /// drive pot, the solver stays finite and the stage stays bounded.
    #[test]
    fn bounded_when_slammed() {
        for pos in [0.0f32, 10.0] {
            let mut p = prepared();
            let mut worst = 0.0f32;
            for k in 0..20_000 {
                let x = if k % 2 == 0 { 1e6 } else { -1e6 };
                let y = p.step(x);
                assert!(y.is_finite(), "non-finite output at drive {pos}");
                worst = worst.max(y.abs());
            }
            // The output is a diode clamp plus the input's own feed-through;
            // what matters is that it does not run away.
            assert!(worst < 1e7, "unbounded at drive {pos}: {worst:e}");
        }
    }

    /// Rate independence: the stage is a circuit, so its small-signal response
    /// must not depend on how finely it is sampled.
    #[test]
    fn the_response_holds_across_sample_rates() {
        for base in [44_100.0f32, 48_000.0, 96_000.0] {
            let os = 4.0 * base;
            let mut p = TsWdf::new();
            p.prepare(base, os);
            let amp = 1e-3f32;
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
