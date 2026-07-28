//! **sd1** — a *white-box* Boss SD-1 "Super OverDrive": the op-amp overdrive
//! stage modelled as a Wave Digital Filter with the diodes in the **feedback
//! loop** and an **asymmetric** clipper (2 diodes one way, 1 the other). This
//! is the deep-water line's second circuit (PRD 021 / ADR 029) and the `v2`
//! that PRD 020 / ADR 028 deferred: [`super::screamer`] modelled the Tube
//! Screamer's clipping as a *shunt* RC-diode network (a faithful reduction) and
//! its diodes are matched; here the topology is the real thing.
//!
//! # The feedback topology, reduced by the ideal op-amp
//!
//! A non-inverting op-amp holds `V(−) = V(+) = Vin`. That forces a current
//! `I_g` through the gain-setting leg (`R_gain` in series with `C_g`, to
//! ground) which depends only on `Vin` and the leg's state — a linear
//! sub-problem. KCL at the inverting node then forces that same `I_g` into the
//! feedback network `[R_f ‖ C_f ‖ diodes]`; solving that current-driven network
//! for its voltage `V_fb` is the one nonlinear step (a WDF root, [`AsymDiode`]),
//! and the output is `Vout = Vin + V_fb`. Two consequences fall straight out of
//! the topology, where `screamer` had to fake them:
//!
//! * **the dry signal always passes** (`+Vin`, gain ≥ 1) — an op-amp overdrive
//!   never becomes a fuzz, no hand-summed dry path needed;
//! * **the mid-hump is intrinsic** — `C_g` rolls the loop gain down toward 1
//!   below its (drive-dependent) corner, so lows stay clean while mids break
//!   up, with no separate input high-pass;
//! * `C_f` (51 pF) sits across the diodes and rounds the hardest highs.
//!
//! The nonlinear feedback node uses [`crate::blocks::wdf`]; the linear gain leg
//! is a direct bilinear RC kept here (circuit-specific voicing, like
//! `screamer`'s series RC). `ts9` and `screamer` are untouched — the three are
//! the deliberate A/B: memoryless curve vs shunt-WDF vs feedback-WDF.

use lh_core::{EffectDesc, ParamDesc};

use super::{Circuit, OnePole, knob, lp_coeff};
use crate::blocks::wdf::{AsymDiode, Capacitor, Parallel, Resistor, Wdf};

static PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "sd1",
    name: "Super Drive",
    params: &PARAMS,
};

/// Feedback resistor (the diodes clip across it). Sized so the op-amp gain
/// `1 + R_f/R_gain` pushes a nominal guitar level past the diode drops across
/// the drive range — a hotter op-amp overdrive than the TS's 51 kΩ, and it
/// drops `C_f`'s corner to a musical ≈14 kHz.
const R_F: f32 = 120_000.0;
/// Feedback capacitor across the diodes — rounds the hardest highs.
const C_F: f32 = 51e-12;
/// Fixed series resistor in the gain leg, ahead of the drive pot.
const R_GAIN_MIN: f32 = 4_700.0;
/// Drive pot (SD-1's 100 kΩ — tighter than the TS's 500 kΩ). Drive 10 shorts
/// it out (max gain); drive 0 is the full resistance (near-unity).
const R_DRIVE_MAX: f32 = 100_000.0;
/// Gain-leg cap: sets the drive-dependent low-frequency gain roll-off that is
/// the mid-hump (corner ≈ 32 Hz at drive 0 … ≈ 720 Hz at drive 10).
const C_G: f32 = 0.047e-6;
// 1N4148 SPICE-representative junction parameters, 2 diodes forward / 1 reverse
// — the SD-1's asymmetric clipper (even harmonics, unlike the matched TS pair).
const IS: f32 = 2.52e-9;
const N: f32 = 1.75;
const VT: f32 = 25.85e-3;
const M_FWD: f32 = 2.0;
const M_REV: f32 = 1.0;
/// Calibrated so drive 5 / tone 5 / level 6 lands near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.26;

/// Gain-pot resistance for a drive position `0..10` (audio-ish taper): the pot
/// shorts out toward drive 10, so gain climbs as resistance falls.
#[inline]
fn drive_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    R_DRIVE_MAX * (1.0 - n) * (1.0 - n)
}

/// The feedback network as a WDF tree: the feedback resistor in parallel with
/// the capacitor across it. The asymmetric diode pair is the root.
type FeedbackTree = Parallel<Resistor, Capacitor>;

pub(super) struct Sd1 {
    tone_lp: OnePole,
    dc: OnePole,
    c723: f32,
    c_dc: f32,
    /// The WDF feedback network: a capacitor across the feedback resistor with
    /// the asymmetric diode pair as the nonlinear root.
    fb: FeedbackTree,
    diode: AsymDiode,
    /// `2·C_g·fs` — the gain leg's bilinear cap admittance.
    g2: f32,
    // Gain-leg bilinear state: last cap voltage and last leg current.
    v_cg: f32,
    i_g: f32,
}

impl Sd1 {
    pub(super) fn new() -> Self {
        Self {
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c723: 0.0,
            c_dc: 0.0,
            fb: Parallel::new(Resistor::new(R_F), Capacitor::new(C_F, 4.0 * 48_000.0)),
            diode: AsymDiode::new(IS, N, VT, M_FWD, M_REV),
            g2: 2.0 * C_G * 4.0 * 48_000.0,
            v_cg: 0.0,
            i_g: 0.0,
        }
    }

    /// One oversampled sample through the feedback op-amp stage. `r_drive` is
    /// the gain pot's resistance this sample. Returns `Vout = Vin + V_fb`.
    #[inline]
    fn clip(&mut self, vin: f32, r_drive: f32) -> f32 {
        // Gain leg (series R_gain + C_g to ground), driven by Vin held at the
        // inverting node. Bilinear-exact: solve the cap voltage and the leg
        // current this sample, then advance the state.
        let r_gain = R_GAIN_MIN + r_drive;
        let rg2 = r_gain * self.g2;
        let v_cg = (vin + rg2 * self.v_cg + r_gain * self.i_g) / (1.0 + rg2);
        let i_g = self.g2 * (v_cg - self.v_cg) - self.i_g;
        self.v_cg = v_cg;
        self.i_g = i_g;

        // That current is forced into the feedback network [R_f ‖ C_f ‖ diodes];
        // solve the nonlinear node voltage V_fb (the drop from Vout to Vin).
        //
        // An *ideal* current source has no WDF one-port form — its port
        // resistance is infinite — but at the root it needs none: injecting `I`
        // into the node shifts the incident wave by `R·I` and leaves `R` alone.
        let a = self.fb.reflected();
        let r = self.fb.resistance();
        let (v_fb, _b) = self.diode.solve(a + r * i_g, r);
        // The network below is handed the wave its *node voltage* implies. The
        // shift belongs to the root's equation, not to the tree, so the
        // unshifted `a` is what `2v − a` is built from here.
        self.fb.incident(2.0 * v_fb - a);

        vin + v_fb
    }
}

impl Circuit for Sd1 {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c723 = lp_coeff(723.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        // The reactive elements are discretized at the rate the solver runs at,
        // so the tree's port resistances are rebuilt from the root down.
        self.fb.prepare(os_rate);
        self.fb.calc_impedance();
        self.g2 = 2.0 * C_G * os_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.tone_lp.reset();
        self.dc.reset();
        self.fb.reset();
        self.diode.reset();
        self.v_cg = 0.0;
        self.i_g = 0.0;
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        for (s, d) in block.iter_mut().zip(drive) {
            *s = self.clip(*s, drive_ohms(*d));
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        for (s, t) in block.iter_mut().zip(tone) {
            let x = *s;
            let lp = self.tone_lp.lp(x, self.c723);
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
    use crate::blocks::wdf::parallel_root_with_source;

    const OS: f32 = 4.0 * 48_000.0;

    /// **The golden.** The pre-framework implementation, kept verbatim as
    /// PRD 021 shipped it: the bilinear gain leg, then a hand-reduced
    /// current-injected parallel node feeding the asymmetric root.
    struct Legacy {
        c_f: Capacitor,
        diode: AsymDiode,
        g2: f32,
        v_cg: f32,
        i_g: f32,
    }

    impl Legacy {
        fn new() -> Self {
            let mut c_f = Capacitor::new(C_F, OS);
            c_f.calc_impedance();
            Self {
                c_f,
                diode: AsymDiode::new(IS, N, VT, M_FWD, M_REV),
                g2: 2.0 * C_G * OS,
                v_cg: 0.0,
                i_g: 0.0,
            }
        }

        fn clip(&mut self, vin: f32, r_drive: f32) -> f32 {
            let r_gain = R_GAIN_MIN + r_drive;
            let rg2 = r_gain * self.g2;
            let v_cg = (vin + rg2 * self.v_cg + r_gain * self.i_g) / (1.0 + rg2);
            let i_g = self.g2 * (v_cg - self.v_cg) - self.i_g;
            self.v_cg = v_cg;
            self.i_g = i_g;

            let a_cf = self.c_f.reflected();
            let (a_root, r_root) =
                parallel_root_with_source(&[(1.0 / R_F, 0.0), (self.c_f.conductance(), a_cf)], i_g);
            let (v_fb, _b) = self.diode.solve(a_root, r_root);
            self.c_f.incident(2.0 * v_fb - a_cf);
            vin + v_fb
        }
    }

    /// Phase 03 §2.4: the rewrite onto `Parallel<Resistor, Capacitor>` keeps
    /// the ideal virtual short untouched and changes nothing audible. The
    /// interesting part is the forced current: an ideal current source has no
    /// one-port form, so the framework version shifts the root's incident wave
    /// by `R·I` and hands the tree `2v − a` built from the *unshifted* wave.
    /// This test is what proves those two spellings are the same circuit.
    #[test]
    fn the_framework_rewrite_is_numerically_equivalent() {
        let mut new = prepared();
        let mut old = Legacy::new();

        let mut worst = 0.0f32;
        for k in 0..200_000 {
            let t = k as f32 / OS;
            // Sweep the drive pot as well — the gain leg's resistance moves,
            // which is where a stale cached impedance would show up.
            let pos = 10.0 * (0.5 + 0.5 * (0.7 * t).sin());
            let r = drive_ohms(pos);
            let amp = 0.02 * (1.0 + 200.0 * (k as f32 / 200_000.0));
            let x = amp * (std::f32::consts::TAU * (150.0 + 6_000.0 * t) * t).sin();
            let d = (new.clip(x, r) - old.clip(x, r)).abs();
            worst = worst.max(d);
        }
        assert!(worst < 1e-4, "worst |Δ| = {worst:e} V");
    }

    fn prepared() -> Sd1 {
        let mut s = Sd1::new();
        s.prepare(48_000.0, OS);
        s
    }

    /// Run a sine of amplitude `amp` at `f` through the WDF core at drive
    /// position `pos`, returning the settled second half.
    fn clip_sine(amp: f32, f: f32, pos: f32, n: usize) -> Vec<f32> {
        let mut s = prepared();
        let r = drive_ohms(pos);
        (0..n)
            .map(|k| s.clip(amp * (std::f32::consts::TAU * f * k as f32 / OS).sin(), r))
            .skip(n / 2)
            .collect()
    }

    /// Fraction of a buffer's energy that is *not* at the fundamental `f`.
    fn harmonic_frac(buf: &[f32], f: f32) -> f64 {
        let n = buf.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in buf.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(f) * i as f64 / f64::from(OS);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        let fund_rms = ((cs * 2.0 / n).powi(2) + (cc * 2.0 / n).powi(2)).sqrt() / 2f64.sqrt();
        let total = (buf.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / n).sqrt();
        (total.powi(2) - fund_rms.powi(2)).max(0.0).sqrt() / total
    }

    /// The white-box payoff at the core: the gain leg's `C_g` rolls loop gain
    /// down at low frequencies, so at the same input amplitude a mid note is
    /// driven into the diodes harder than a low note — the mid-hump grown from
    /// the circuit, which a memoryless curve cannot do. (`screamer` shows the
    /// dual effect — its shunt cap softens *highs*; here the gain leg lifts
    /// *mids*.)
    #[test]
    fn gain_leg_makes_clipping_frequency_dependent() {
        // At a nominal guitar amplitude the gain leg keeps a low note below the
        // diode knee while a mid note is driven past it.
        let low = harmonic_frac(&clip_sine(0.12, 100.0, 7.0, 1 << 15), 100.0);
        let mid = harmonic_frac(&clip_sine(0.12, 1_000.0, 7.0, 1 << 15), 1_000.0);
        assert!(
            mid > low * 1.2,
            "mids must break up more than lows: 100 Hz {low:.3} vs 1 kHz {mid:.3}"
        );
    }

    /// The 2-diode / 1-diode feedback clipper is asymmetric: a symmetric sine
    /// comes out with a real DC offset (one polarity clamps earlier). The
    /// matched `screamer` core has ~none.
    #[test]
    fn core_clip_is_asymmetric() {
        // 192 kHz / 200 Hz = 960 samples/cycle; the settled half is 100 cycles.
        let tail = clip_sine(3.0, 200.0, 8.0, 192_000);
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        let pk = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(pk > 0.3, "should be clipping, peak {pk}");
        assert!(
            mean.abs() > 0.01,
            "asymmetric clip must offset DC, mean {mean}"
        );
    }

    /// Silence in → exact silence out at the core (the solver's fixed point at
    /// `a = 0` is `v = 0`, and every reactive state stays zero).
    #[test]
    fn core_silence_in_silence_out() {
        let mut s = prepared();
        let r = drive_ohms(7.0);
        for _ in 0..1000 {
            assert_eq!(s.clip(0.0, r), 0.0);
        }
    }
}
