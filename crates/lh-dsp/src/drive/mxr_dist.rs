//! **mxr-dist** — the MXR Distortion+: the same op-amp as its Screamer-family
//! cousins, clipping in a completely different place (PRD 028; Tone Revolution
//! phase 04).
//!
//! [`super::ts_wdf`] and [`super::zendrive`] put their diodes **in the feedback
//! loop**, where the op-amp fights them and the result is a soft, negotiated
//! knee. This one amplifies first — up to 213× — and then throws the result at a
//! diode pair **shunted to ground** at the output. Nothing negotiates. The stage
//! is a limiter with a lot of gain in front of it, and that is why a Distortion+
//! sounds hard and compressed where a Screamer sounds like it is leaning.
//!
//! Structurally that shows up as *which port is adapted*: the up port has to
//! face the nonlinearity, so this pedal uses
//! [`NON_INVERTING_OUT_PORTS`](crate::blocks::wdf::NON_INVERTING_OUT_PORTS) —
//! the same amplifier and the same four ports as the Screamer family, with the
//! output leading out and the feedback resistor demoted to an ordinary child.
//!
//! # Where the character comes from
//!
//! - **Mid-forward.** `C3` (47 nF) sits under the gain leg, so below
//!   `1/(2π·R_leg·C3)` — 720 Hz wide open — the leg's impedance climbs and the
//!   gain falls back toward unity. Same mechanism as a Screamer's mid-hump, but
//!   with far more gain above the corner, so the hump is a cliff.
//! - **Hard, and it stays hard.** With 213× available and a clipper at a third
//!   of a volt, anything a guitar produces is deep into the clamp. Halving the
//!   input barely changes the output — the opposite of
//!   [`super::zendrive`], and pinned against it.
//! - **Band-limited.** The op-amp is a 741: 1 MHz gain-bandwidth, so at the top
//!   of the Dist sweep the loop has only ~5 of gain to spare at 1 kHz and less
//!   above it. The finite-gain model earns its keep here — the stage genuinely
//!   cannot deliver the 213× its resistors ask for, and it runs out of it
//!   fastest exactly where the ear notices.
//!
//! # Faceplate
//!
//! Dist / Diode / Output. The real pedal has two knobs; the third is the one
//! version difference worth having, since early units clipped with **germanium**
//! and later ones with silicon, and they are not the same box.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Circuit, OnePole, knob, lp_coeff};
use crate::blocks::wdf::{
    CapacitiveVoltageSource, Capacitor, DiodePair, JEl, Junction, NON_INVERTING_NODES,
    NON_INVERTING_OUT_PORTS, Parallel, RType, Resistor, ResistorCapacitorSeries, Series, Wdf,
    non_inverting_els,
};

/// The version split, as `(Is, n)`. Early Distortion+ units clipped with
/// germanium — softer knee, lower clamp, the ones people hunt for; later ones
/// with silicon, which is louder and squarer. Same `(Is, n)` convention as
/// [`super::ts_wdf`] (ADR 033): germanium is high `Is` *and* near-unity `n`.
static DIODE_LABELS: [&str; 2] = ["1N34", "1N914"];
static DIODE_MODEL: [(f32, f32); 2] = [(2.0e-7, 1.28), (2.52e-9, 1.75)];
static DIODE_RANGE: Range = Range::Stepped {
    labels: &DIODE_LABELS,
};

static PARAMS: [ParamDesc; 3] = [
    knob("dist", "Dist", 5.0, 20.0),
    ParamDesc {
        key: "diode",
        name: "Diode",
        unit: "",
        range: DIODE_RANGE,
        default: 0.0,
        smoothing_ms: 0.0,
    },
    knob("level", "Output", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "mxr-dist",
    name: "Dist Plus",
    params: &PARAMS,
};

// --- the netlist ---

/// Input series resistor.
const R1: f32 = 10e3;
/// Input coupling capacitor. With [`R2`] this is the 16 Hz high-pass.
const C2: f32 = 10e-9;
/// Bias resistor on the non-inverting pin (referenced to ground here — see the
/// note on supply rails below).
const R2: f32 = 1e6;
/// Fixed part of the gain leg.
const R3: f32 = 4.7e3;
/// Gain-leg capacitor — the mid-forward corner the Dist knob sweeps.
const C3: f32 = 47e-9;
/// Feedback resistor. Fixed: on this pedal the pot is in the *leg*.
const R4: f32 = 1e6;
/// Output series resistor.
const R5: f32 = 10e3;
/// Output coupling capacitor.
const C4: f32 = 1e-6;
/// Output load, and where the clipped voltage is read.
const R_OUT: f32 = 10e3;
/// Output shunt capacitor.
const C5: f32 = 1e-9;

/// Op-amp open-loop gain at 1 kHz — a 741 (1 MHz gain-bandwidth). Low, and
/// legitimately so: this is a 1970s pedal built around a 1970s part, and the
/// gain it runs out of is part of the sound (ADR 033).
const AG: f32 = 1.0e3;
/// Op-amp differential input resistance (741 typical).
const RI: f32 = 2e6;
/// Op-amp open-loop output resistance (741 typical).
const RO: f32 = 75.0;

/// Thermal voltage at room temperature.
const VT: f32 = 0.02585;

static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO);

/// The Screamer family's amplifier, adapted at the output because that is where
/// this pedal's diodes are. See [`NON_INVERTING_OUT_PORTS`].
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_OUT_PORTS,
};

/// Oversampled samples between impedance rebuilds; see [`super::ts_wdf`].
const REBUILD: usize = 64;
const DC_HZ: f32 = 10.0;
/// Calibrated so dist 5 / output 6 lands near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.44;

/// Stage gain at the bottom and top of the Dist sweep. The top is the circuit's
/// own limit, `1 + R4/R3`; the bottom is where the 1 MΩ pot bottoms it out.
const GAIN_MIN: f32 = 2.0;
const GAIN_MAX: f32 = 1.0 + R4 / R3;

/// Gain-leg resistance for a Dist position 0..10.
///
/// The pot is in the leg, so gain is `1 + R4/R_leg` — hyperbolic in the pot,
/// which would cram the whole usable range into the last tenth of the knob. So
/// the taper is defined on the *gain* instead: geometric from [`GAIN_MIN`] to
/// [`GAIN_MAX`], inverted back into a resistance. The knob is then linear in dB
/// of stage gain, and both ends are still the real component values.
#[inline]
fn leg_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    let g = GAIN_MIN * (GAIN_MAX / GAIN_MIN).powf(n);
    (R4 / (g - 1.0)).max(R3)
}

/// `Vin` through `C2` and `R1`, loaded by the bias resistor.
type InputLeg = Parallel<Resistor, Series<Resistor, CapacitiveVoltageSource>>;
/// The op-amp junction: feedback, input leg and gain leg as children; the
/// output is the adapted port.
type OpAmpNode = RType<4, 3, (Resistor, InputLeg, ResistorCapacitorSeries)>;
/// The output network, and what the clipper sees. `R_OUT`, `C5` and the series
/// output leg all meet at one node — so the diode voltage *is* the voltage
/// across `R_OUT`, and the output tap costs nothing.
type ClipTree = Parallel<Capacitor, Parallel<Resistor, Series<ResistorCapacitorSeries, OpAmpNode>>>;

pub(super) struct MxrDist {
    tree: ClipTree,
    diodes: DiodePair,
    /// Gain-leg resistance the tree was last built for (settled-skip).
    leg_ohms: f32,
    dc: OnePole,
    c_dc: f32,
}

impl MxrDist {
    pub(super) fn new() -> Self {
        const SR0: f32 = 4.0 * 48_000.0;
        Self {
            tree: Parallel::new(
                Capacitor::new(C5, SR0),
                Parallel::new(
                    Resistor::new(R_OUT),
                    Series::new(
                        ResistorCapacitorSeries::new(R5, C4, SR0),
                        RType::new(
                            &JUNCTION,
                            (
                                Resistor::new(R4),
                                Parallel::new(
                                    Resistor::new(R2),
                                    Series::new(
                                        Resistor::new(R1),
                                        CapacitiveVoltageSource::new(C2, SR0),
                                    ),
                                ),
                                ResistorCapacitorSeries::new(leg_ohms(5.0), C3, SR0),
                            ),
                        ),
                    ),
                ),
            ),
            diodes: DiodePair::new(DIODE_MODEL[0].0, DIODE_MODEL[0].1, VT),
            leg_ohms: leg_ohms(5.0),
            dc: OnePole::default(),
            c_dc: 0.0,
        }
    }

    #[inline]
    fn set_input(&mut self, v: f32) {
        self.tree
            .port2_mut()
            .port2_mut()
            .port2_mut()
            .ports_mut()
            .1
            .port2_mut()
            .port2_mut()
            .set_voltage(v);
    }

    /// One oversampled sample. The clipper's own node voltage *is* the output —
    /// `R_OUT` hangs on the same node.
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        self.set_input(x);
        let a = self.tree.reflected();
        let (v, b) = self.diodes.solve(a, self.tree.resistance());
        self.tree.incident(b);
        v
    }

    fn retune(&mut self, dist_pos: f32) {
        let leg = leg_ohms(dist_pos);
        if leg != self.leg_ohms {
            self.leg_ohms = leg;
            self.tree
                .port2_mut()
                .port2_mut()
                .port2_mut()
                .ports_mut()
                .2
                .set_ohms(leg);
            self.tree.calc_impedance();
        }
    }
}

impl Circuit for MxrDist {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_dc = lp_coeff(DC_HZ, base_rate);
        self.tree.prepare(os_rate);
        self.tree.calc_impedance();
        self.reset();
    }

    fn reset(&mut self) {
        self.tree.reset();
        self.diodes.reset();
        self.dc.reset();
    }

    fn set_shape(&mut self, index: usize) {
        let (is, n) = DIODE_MODEL[index.min(DIODE_MODEL.len() - 1)];
        self.diodes.set_params(is, n, VT);
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

    /// No tone control — the pedal has none, and its voicing is already in the
    /// circuit (the gain leg's corner going in, `C5` and the output network
    /// coming out). Makeup and a DC blocker, nothing else.
    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
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

    fn prepared() -> MxrDist {
        let mut p = MxrDist::new();
        p.prepare(48_000.0, OS);
        p
    }

    fn run(p: &mut MxrDist, amp: f32, f: f32, dist: f32, n: usize) -> Vec<f32> {
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

    fn harmonic_frac(buf: &[f32], f: f32) -> f64 {
        let fund = mag_at(buf, f);
        let total = (buf.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / buf.len() as f64)
            .sqrt()
            * std::f64::consts::SQRT_2;
        ((total * total - fund * fund).max(0.0)).sqrt() / total.max(1e-12)
    }

    /// Small-signal gain at `f`, Dist at `pos`. The amplitude has to keep the
    /// *output* under the clipper's knee, and this stage has up to 213× — hence
    /// a microvolt-scale probe.
    fn measured_gain(f: f32, pos: f32) -> f64 {
        const AMP: f32 = 1e-5;
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

    /// |H(jω)| of the **analog** pedal, hand-solved in three stages:
    /// input high-pass → finite-gain non-inverting amp → output network.
    ///
    /// The op-amp's output impedance is left out of the last stage: feedback
    /// divides `RO` by the loop gain, leaving ~13 Ω in series with `R5`'s 10 kΩ.
    fn analog_gain(w: f64, pos: f32) -> f64 {
        // Input: Vin through R1 + 1/(jωC2), loaded by R2 to ground.
        let z_in = (f64::from(R1), -1.0 / (w * f64::from(C2)));
        let r2 = (f64::from(R2), 0.0);
        let h_in = cdiv(r2, cadd(r2, z_in));

        // Amplifier: β = Zg/(Zg + R4), gain = Ag/(1 + Ag·β).
        let zg = (f64::from(leg_ohms(pos)), -1.0 / (w * f64::from(C3)));
        let beta = cdiv(zg, cadd(zg, (f64::from(R4), 0.0)));
        let ag = f64::from(AG);
        let h_amp = cdiv((ag, 0.0), cadd((1.0, 0.0), cmul((ag, 0.0), beta)));

        // Output: R5 + 1/(jωC4) into R_OUT ‖ C5 ‖ the diodes' small-signal Rd.
        let (is, n) = DIODE_MODEL[0];
        let rd = f64::from(n * VT) / (2.0 * f64::from(is));
        let z_sh = cdiv(
            (1.0, 0.0),
            (1.0 / f64::from(R_OUT) + 1.0 / rd, w * f64::from(C5)),
        );
        let z_ser = (f64::from(R5), -1.0 / (w * f64::from(C4)));
        let h_out = cdiv(z_sh, cadd(z_sh, z_ser));

        cabs(cmul(cmul(h_in, h_amp), h_out))
    }

    fn prewarp(f: f32) -> f64 {
        2.0 * f64::from(OS) * (std::f64::consts::PI * f64::from(f) / f64::from(OS)).tan()
    }

    /// **The independent check on the whole circuit**, and the one that settles
    /// the two structural questions this pedal raised: that adapting the
    /// junction at its *output* port is sound, and that the output network's
    /// nested series composition is the flat series chain the schematic shows.
    ///
    /// Below the diodes' knee the pedal is linear, so its measured response must
    /// match hand-solved AC analysis of the same netlist — input high-pass,
    /// finite-gain amplifier, output network — with nothing in common with the
    /// implementation but the component values.
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

    /// The root really is solved — checked the way PRD 022 set up, against the
    /// **Newton oracle** rather than against a relative residual.
    ///
    /// That choice matters here. `DiodePair::solve` is a closed-form
    /// *approximation* with a roughly fixed absolute error on the node voltage,
    /// so a residual measured relative to the incident wave explodes on quiet
    /// samples while saying nothing about accuracy where it is audible. This
    /// pedal's germanium pair is also stiffer than the silicon ones elsewhere in
    /// the family (`n·Vt` 33 mV against 49 mV), which sharpens the crossover the
    /// approximation smooths — worth measuring rather than assuming.
    #[test]
    fn the_closed_form_root_tracks_the_newton_oracle() {
        let mut p = prepared();
        let (is, n) = DIODE_MODEL[0];
        let (is, n_vt) = (f64::from(is), f64::from(n * VT));
        let mut worst_gap = 0.0f64;
        let mut worst_oracle = 0.0f64;
        for k in 0..50_000 {
            let t = k as f32 / OS;
            let amp = 0.0005 * (1.0 + 1_000.0 * (k as f32 / 50_000.0));
            let x = amp * (std::f32::consts::TAU * (140.0 + 3_500.0 * t) * t).sin();
            p.set_input(x);
            let a = p.tree.reflected();
            let r = p.tree.resistance();
            let (v, b) = p.diodes.solve(a, r);
            let (v_ref, _) = p.diodes.solve_newton(a, r);
            p.tree.incident(b);
            worst_gap = worst_gap.max(f64::from(v - v_ref).abs());
            // The oracle itself must satisfy the equation to its own tolerance.
            let i = 2.0 * is * (f64::from(v_ref) / n_vt).sinh();
            worst_oracle =
                worst_oracle.max((f64::from(a) - (f64::from(v_ref) + f64::from(r) * i)).abs());
        }
        assert!(
            worst_oracle < 1e-6,
            "the Newton oracle must solve the equation, residual {worst_oracle:e} V"
        );
        assert!(
            worst_gap < 1e-3,
            "closed form vs oracle: worst |Δv| = {worst_gap:e} V"
        );
    }

    /// **Mid-forward**, and it comes from `C3` under the gain leg: below its
    /// corner the leg's impedance climbs and the stage falls back toward unity,
    /// so the bass never gets the gain the mids do. Measured in the linear
    /// regime, where it is purely the topology talking.
    #[test]
    fn the_gain_leg_makes_it_mid_forward() {
        let low = measured_gain(80.0, 8.0);
        let mid = measured_gain(1_000.0, 8.0);
        assert!(
            mid > 3.0 * low,
            "mids must tower over lows: 80 Hz {low:.2}× vs 1 kHz {mid:.2}×"
        );
    }

    /// **The character pin, stated against [`super::super::zendrive`].** That
    /// pedal clips inside the loop against a high, soft knee; this one amplifies
    /// 200× and slams a germanium pair to ground at a third of a volt. So at a
    /// level where the transparent pedal is *still perfectly clean*, this one is
    /// already breaking up — same knob position, same input.
    ///
    /// The comparison is the test. An absolute distortion figure would drift
    /// with any voicing change, and at a cranked setting everything clips;
    /// "dirty where the transparent one is clean" is the claim that actually
    /// separates the two circuits.
    #[test]
    fn it_breaks_up_where_the_zendrive_is_still_clean() {
        const AMP: f32 = 0.02;
        let mut p = prepared();
        let mine = harmonic_frac(&run(&mut p, AMP, 440.0, 5.0, 1 << 15), 440.0);

        let n = 1 << 15;
        let mut z = super::super::zendrive::ZenDrive::new();
        z.prepare(48_000.0, OS);
        let traj = vec![5.0f32; n];
        let mut buf: Vec<f32> = (0..n)
            .map(|k| AMP * (std::f32::consts::TAU * 440.0 * k as f32 / OS).sin())
            .collect();
        z.shape(&mut buf, &traj);
        let zen = harmonic_frac(&buf[n / 2..], 440.0);

        assert!(
            zen < 0.01,
            "the reference pedal must still be clean here, got {zen:.4}"
        );
        assert!(
            mine > 10.0 * zen.max(1e-4),
            "…and this one must already be dirty: {mine:.4} vs {zen:.4}"
        );
    }

    /// And once it is in, it *stays* in: this is a limiter with a lot of gain in
    /// front of it, so a 12 dB drop at the input barely moves the output. A
    /// linear stage would give 0.25.
    #[test]
    fn it_compresses_hard() {
        let loud = {
            let mut p = prepared();
            mag_at(&run(&mut p, 0.2, 440.0, 7.0, 1 << 15), 440.0)
        };
        let soft = {
            let mut p = prepared();
            mag_at(&run(&mut p, 0.05, 440.0, 7.0, 1 << 15), 440.0)
        };
        let tracking = soft / loud;
        assert!(
            tracking > 0.6,
            "12 dB in should barely move the output, got {tracking:.3}"
        );
    }

    /// Wound up on a guitar-level signal it is not "driven", it is squared: most
    /// of the output energy is no longer at the fundamental.
    #[test]
    fn it_clips_hard_at_playing_level() {
        let mut p = prepared();
        let h = harmonic_frac(&run(&mut p, 0.15, 440.0, 8.0, 1 << 15), 440.0);
        assert!(h > 0.3, "expected a squared wave, got {h:.3} inharmonic");
    }

    /// The Diode knob is the version difference: germanium clamps lower than
    /// silicon, so the early-unit setting is quieter and breaks up sooner.
    #[test]
    fn the_diode_selector_picks_the_version() {
        let peak_for = |index: usize| {
            let mut p = prepared();
            p.set_shape(index);
            let y = run(&mut p, 0.1, 440.0, 7.0, 1 << 14);
            y.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let germanium = peak_for(0);
        let silicon = peak_for(1);
        assert!(
            germanium < 0.8 * silicon,
            "germanium must clamp lower: {germanium:.3} V vs silicon {silicon:.3} V"
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
            let mut p = MxrDist::new();
            p.prepare(base, os);
            let amp = 1e-5f32;
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
