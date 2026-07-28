//! **screamer** — a *white-box* Tube-Screamer clipping stage: the RC +
//! antiparallel-diode network solved as a Wave Digital Filter
//! ([`crate::blocks::wdf`]), the project's first circuit-level model (white
//! paper §6 deep water, first topic; PRD 020 / ADR 028).
//!
//! Where [`super::ts9`] approximates the clip with a memoryless curve
//! (`x/√(1+x²)`), this pedal solves the actual diode equation
//! `i = 2·Is·sinh(v/nVt)` every oversampled sample. The clip therefore sits
//! behind a *reactive* node — a shunt capacitor across the diodes — so the
//! clipping threshold moves with frequency and transient the way the real
//! network does (a memoryless shaper clips every frequency identically). This
//! is the whole point of the exercise; `ts9` stays as the A/B reference.
//!
//! Voicing around the clip mirrors the TS: the gained path is high-passed at
//! 720 Hz into the clipper and summed with the unity dry path for the classic
//! mid-hump, then the one-pole dark↔bright tone tilt with makeup.

use lh_core::{EffectDesc, ParamDesc};

use super::{Circuit, OnePole, knob, lp_coeff};
use crate::blocks::wdf::{Capacitor, DiodePair, Parallel, ResistiveVoltageSource, Wdf};

static PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "screamer",
    name: "Screamer",
    params: &PARAMS,
};

/// Series resistance between the op-amp output and the clipping node.
const R_SERIES: f32 = 2200.0;
/// Node-to-ground capacitor in parallel with the diodes. With `R_SERIES` it
/// forms a ~3.3 kHz corner: above it the cap diverts current from the diodes,
/// so highs clip softer and the top darkens as the note climbs.
const C_SHUNT: f32 = 22e-9;
// 1N4148 SPICE-representative junction parameters.
const IS: f32 = 2.52e-9;
const N: f32 = 1.75;
const VT: f32 = 25.85e-3;
/// Calibrated so drive 5 / tone 5 / level 6 lands near unity loudness
/// (`modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.20;

/// Feedback resistance for a drive-pot position 0..10 (51 kΩ series plus the
/// 500 kΩ pot, audio taper) — the TS op-amp gain law, shared with `ts9`.
#[inline]
fn feedback_ohms(pos: f32) -> f32 {
    let n = pos * 0.1;
    51_000.0 + 500_000.0 * n * n
}

/// The clipping network as a WDF tree: the op-amp's output behind `R_SERIES`
/// (a Thévenin source) in parallel with the shunt capacitor. The diode pair is
/// the root and is not part of the tree.
///
/// This is the whole point of the per-pedal type alias convention (ADR 032) —
/// the circuit's topology is legible in one line, and the compiler flattens it
/// into the same straight-line arithmetic the hand-reduced version had.
type ClipTree = Parallel<ResistiveVoltageSource, Capacitor>;

pub(super) struct Screamer {
    hp720: OnePole,
    tone_lp: OnePole,
    dc: OnePole,
    c720: f32,
    c723: f32,
    c_dc: f32,
    /// The WDF clipping stage: a shunt capacitor and an antiparallel diode
    /// pair at the parallel node, driven through `R_SERIES`.
    tree: ClipTree,
    diode: DiodePair,
}

impl Screamer {
    pub(super) fn new() -> Self {
        Self {
            hp720: OnePole::default(),
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c720: 0.0,
            c723: 0.0,
            c_dc: 0.0,
            tree: Parallel::new(
                ResistiveVoltageSource::new(R_SERIES),
                Capacitor::new(C_SHUNT, 4.0 * 48_000.0),
            ),
            diode: DiodePair::new(IS, N, VT),
        }
    }

    /// One oversampled sample through the WDF shunt clipper. Returns the node
    /// (diode) voltage `v` — the clipped signal.
    #[inline]
    fn clip(&mut self, e: f32) -> f32 {
        self.tree.port1_mut().set_voltage(e);
        let a = self.tree.reflected();
        let (v, b) = self.diode.solve(a, self.tree.resistance());
        self.tree.incident(b);
        v
    }
}

impl Circuit for Screamer {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c720 = lp_coeff(720.0, os_rate);
        self.c723 = lp_coeff(723.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        // The capacitor is discretized at the rate its solver runs at, so the
        // tree's port resistances have to be rebuilt from the root down.
        self.tree.prepare(os_rate);
        self.tree.calc_impedance();
        self.reset();
    }

    fn reset(&mut self) {
        self.hp720.reset();
        self.tone_lp.reset();
        self.dc.reset();
        self.tree.reset();
        self.diode.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        for (s, d) in block.iter_mut().zip(drive) {
            let x = *s;
            // The gained path: input high-passed at 720 Hz, then the op-amp
            // gain. Lows bypass the clipper via the dry sum → the mid-hump.
            let hp = x - self.hp720.lp(x, self.c720);
            let g = 1.0 + feedback_ohms(*d) / 4_700.0;
            let clipped = self.clip(g * hp);
            *s = x + clipped;
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
    use crate::blocks::wdf::parallel_root;

    const OS: f32 = 4.0 * 48_000.0;

    /// **The golden.** The pre-framework implementation, kept verbatim: a
    /// hand-reduced parallel node feeding the diode root, exactly as PRD 020
    /// shipped it. `the_framework_rewrite_is_numerically_equivalent` holds the
    /// composable version to it, so "the refactor changed no sound" is a
    /// measured claim rather than an assurance.
    fn legacy_clip(cap: &mut Capacitor, diode: &mut DiodePair, e: f32) -> f32 {
        let a1 = cap.reflected();
        let (a_root, r_root) = parallel_root(&[(1.0 / R_SERIES, e), (cap.conductance(), a1)]);
        let (v, _b) = diode.solve(a_root, r_root);
        cap.incident(2.0 * v - a1);
        v
    }

    /// Phase 03 §2.4: the rewrite onto `Parallel<ResistiveVoltageSource,
    /// Capacitor>` is a *strictly equivalent* refactor. Float operation order
    /// differs (the adaptor forms the weighted average as `a₂ − p(a₂ − a₁)`
    /// where the helper summed `ΣGa/ΣG`), so bit-equality is not promised —
    /// 1e-4 on the node voltage is, across the whole range the stage sees.
    #[test]
    fn the_framework_rewrite_is_numerically_equivalent() {
        let mut new = Screamer::new();
        new.prepare(48_000.0, OS);
        let mut cap = Capacitor::new(C_SHUNT, OS);
        cap.calc_impedance();
        let mut diode = DiodePair::new(IS, N, VT);

        let mut worst = 0.0f32;
        for k in 0..200_000 {
            // Sweep amplitude from below the diode knee to a slammed op-amp
            // output, with a moving frequency so the shunt cap's state is
            // exercised rather than settled.
            let t = k as f32 / OS;
            let amp = 0.02 * (1.0 + 400.0 * (k as f32 / 200_000.0));
            let e = amp * (std::f32::consts::TAU * (150.0 + 6_000.0 * t) * t).sin();
            let d = (new.clip(e) - legacy_clip(&mut cap, &mut diode, e)).abs();
            worst = worst.max(d);
        }
        assert!(worst < 1e-4, "worst |Δv| = {worst:e} V");
    }

    /// Run a sine of amplitude `amp` at `f` through the WDF core, returning the
    /// settled second half (the capacitor transient has died by then).
    fn clip_sine(amp: f32, f: f32, n: usize) -> Vec<f32> {
        let mut s = Screamer::new();
        s.prepare(48_000.0, OS);
        (0..n)
            .map(|k| s.clip(amp * (std::f32::consts::TAU * f * k as f32 / OS).sin()))
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

    /// The white-box payoff, isolated at the WDF core (no input high-pass to
    /// confound it): the shunt capacitor rounds the clipped waveform's edges as
    /// frequency rises — it diverts current from the diodes and slews the
    /// transitions — so a high note breaks up **less** than a low note at the
    /// same drive. A memoryless clipper (`ts9`) has no such frequency
    /// dependence: it would clip both to the same harmonic content.
    #[test]
    fn shunt_cap_makes_clipping_frequency_dependent() {
        let low = harmonic_frac(&clip_sine(2.0, 200.0, 1 << 15), 200.0);
        let high = harmonic_frac(&clip_sine(2.0, 5_000.0, 1 << 15), 5_000.0);
        assert!(
            low > high * 1.2,
            "highs must break up less than lows: 200 Hz {low:.3} vs 5 kHz {high:.3}"
        );
    }

    /// Antiparallel (matched) diodes clip symmetrically: a sine in yields an
    /// odd-harmonic waveform with negligible DC. Averaged over an integer
    /// number of 200 Hz cycles so a partial cycle can't fake an offset.
    #[test]
    fn core_clip_is_symmetric() {
        // 192 kHz / 200 Hz = 960 samples/cycle; the settled half spans exactly
        // 100 cycles.
        let tail = clip_sine(5.0, 200.0, 192_000);
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        let pk = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(pk > 0.3, "should be clipping, peak {pk}");
        assert!(mean.abs() < 2e-3, "symmetric clip has ~no DC, mean {mean}");
    }

    /// Silence in → exact silence out at the core (the solver's fixed point at
    /// `a = 0` is `v = 0`, and the capacitor state stays zero).
    #[test]
    fn core_silence_in_silence_out() {
        let mut s = Screamer::new();
        s.prepare(48_000.0, OS);
        for _ in 0..1000 {
            assert_eq!(s.clip(0.0), 0.0);
        }
    }
}
