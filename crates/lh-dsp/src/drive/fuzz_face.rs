//! **fuzz-face** — Dallas Arbiter Fuzz Face-style germanium fuzz. Two
//! directly-coupled PNP transistors in a shunt-series feedback pair (the
//! classic 2.2 µF in, 33 kΩ / 8.2 kΩ collectors, 100 kΩ bias feedback, Fuzz
//! at Q2's emitter, Volume off Q2's collector). Almost nothing in common with
//! the op-amp/diode pedals in this family — its whole voice comes from how
//! two transistors misbehave.
//!
//! Three behaviors define it, and this model chases all three:
//! - **Asymmetric clipping.** Q1 biases near 1.3 V, nowhere near mid-supply,
//!   so one polarity hits mushy saturation while the other swings all the way
//!   to cutoff. Modelled as a soft `tanh` on the way up and a **hard flat
//!   clamp** on the way down, plus a fixed bias offset: a fat stack of even
//!   *and* odd harmonics, splatty on the attack — not the tidy odd-only of a
//!   symmetric clipper.
//! - **Gating / splutter on the decay.** The germanium Fuzz Face's signature:
//!   a plucked note fuzzes, sustains, then cuts off with a "velcro" splutter
//!   instead of fading smoothly. Its cause in hardware is *blocking
//!   distortion* — the input coupling cap charges on peaks and bleeds slowly,
//!   biasing the stage toward cutoff once the note falls below where it was.
//!   The audible result is that the note gates when its level drops well below
//!   its own recent peak. We reproduce exactly that: a fast envelope measured
//!   against a slow peak-hold, and when the ratio collapses the output gates.
//!   Because it keys on the *ratio*, not an absolute level, a steady quiet
//!   signal (envelope ≈ its own peak) never gates — it just plays.
//! - **Cleans up with the input.** A real Fuzz Face's ~10 kΩ input impedance
//!   makes it a slave to the guitar's volume pot. We don't have the pickup's
//!   source impedance here, but the sonic result — heavy fuzz that dissolves
//!   to near-clean as the input drops — is inherent to a very-high-gain
//!   clipper and comes for free (and the ratio gate leaves it alone).
//!
//! Two knobs, like the hardware: **Fuzz** (gain — no clean floor, it fuzzes
//! all the way down) and **Volume**. Voiced dark and thick (a pre-clip
//! high-cut for the woolly germanium top); no tone control, by design.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 2] = [
    knob("fuzz", "Fuzz", 5.0, 20.0),
    knob("volume", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "fuzz-face",
    name: "Fuzz Face",
    params: &PARAMS,
};

/// Soft "mushy saturation" knee (the swing toward saturation).
const KNEE_POS: f32 = 0.9;
/// Hard "cutoff" clamp (the swing toward the off transistor). Lower than
/// `KNEE_POS` — the strong asymmetry — and *flat* below it.
const KNEE_NEG: f32 = 0.5;
/// Fixed *pre-gain* bias: Q1's ~1.3 V operating point, nowhere near
/// mid-supply. Offsetting the signal before the huge gain shifts the clip's
/// zero-crossing, so the flat-topped square comes out with an asymmetric duty
/// cycle — the even harmonics survive the DC blocker (a post-clip offset would
/// just be a DC level the blocker erases, leaving a symmetric square).
const PRE_BIAS: f32 = 0.02;
/// The note gates once its envelope falls below this fraction of its slowly-
/// bleeding recent peak — the blocking-distortion cutoff, as a ratio so it
/// fires on a fading note but never on a merely-quiet one.
const GATE_FRAC: f32 = 0.25;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.13;

pub(super) struct FuzzFace {
    hp_in: OnePole,
    dark: OnePole,
    dc_os: OnePole,
    /// Gate key: a fast envelope of the input, a slow-bleeding peak-hold of
    /// that envelope, and the smoothed gate gain riding their ratio.
    env: f32,
    peak: f32,
    gate: f32,
    c_sub: f32,
    c_dark: f32,
    c_dc: f32,
    c_env: f32,
    c_peak: f32,
    c_gate: f32,
}

impl FuzzFace {
    pub(super) fn new() -> Self {
        Self {
            hp_in: OnePole::default(),
            dark: OnePole::default(),
            dc_os: OnePole::default(),
            env: 0.0,
            peak: 0.0,
            gate: 0.0,
            c_sub: 0.0,
            c_dark: 0.0,
            c_dc: 0.0,
            c_env: 0.0,
            c_peak: 0.0,
            c_gate: 0.0,
        }
    }
}

impl Circuit for FuzzFace {
    fn prepare(&mut self, _base_rate: f32, os_rate: f32) {
        // Everything runs at the oversampled rate: the clip is brutal and the
        // gating envelope shares its clock.
        self.c_sub = lp_coeff(50.0, os_rate);
        self.c_dark = lp_coeff(5_500.0, os_rate);
        self.c_dc = lp_coeff(10.0, os_rate);
        // Envelope ~4 ms, gate smoothing ~3 ms (declicked), peak-hold bleed
        // ~0.6 s — slower than the note so the ratio falls as it decays, fast
        // enough that the body still plays before the tail gates.
        self.c_env = lp_coeff(40.0, os_rate);
        self.c_peak = lp_coeff(0.27, os_rate);
        self.c_gate = lp_coeff(50.0, os_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp_in.reset();
        self.dark.reset();
        self.dc_os.reset();
        self.env = 0.0;
        self.peak = 0.0;
        self.gate = 0.0;
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // A fuzz is high gain top to bottom: +20 dB (Fuzz down — still dirty,
        // it cleans up from the *guitar*, not this knob) to +55 dB (full
        // splat). Audio taper, powf twice per chunk, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(20.0 + 35.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // Tightening high-pass at 50 Hz (a fuzz's huge gain turns any
            // sub-bass into flub, worst when a boost is stacked in front),
            // then the woolly germanium high-cut feeding the clipper — dark
            // in, so the fuzz is smooth, not fizzy.
            let x = x - self.hp_in.lp(x, self.c_sub);

            // Gate key on the (clean, pre-gain) input: fast envelope vs a
            // slow-bleeding peak-hold. When the note fades well below its
            // recent peak the ratio collapses and the gate shuts — the germ
            // splutter. A steady signal keeps env ≈ peak, so it never gates.
            self.env += self.c_env * (x.abs() - self.env);
            if self.env > self.peak {
                self.peak = self.env;
            } else {
                self.peak -= self.c_peak * self.peak;
            }
            if self.peak < 1e-20 {
                self.peak = 0.0;
            }
            let target = if self.env > GATE_FRAC * self.peak {
                1.0
            } else {
                0.0
            };
            self.gate += self.c_gate * (target - self.gate);

            let xd = self.dark.lp(x, self.c_dark);
            let v = gain.tick() * (xd + PRE_BIAS);
            // Asymmetric clip: mushy soft saturation pushing up, a hard flat
            // cutoff pulling down.
            let clipped = if v >= 0.0 {
                KNEE_POS * (v / KNEE_POS).tanh()
            } else {
                v.max(-KNEE_NEG)
            };
            *s = (clipped - self.dc_os.lp(clipped, self.c_dc)) * self.gate;
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No tone knob — just the output makeup.
        for s in block.iter_mut() {
            *s *= MAKEUP;
        }
    }
}
