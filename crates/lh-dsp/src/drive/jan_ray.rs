//! **jan-ray** — Vemuram Jan Ray-style transparent overdrive. The Jan Ray is
//! a lightly-modified Paul Cochrane *Timmy*: an LM4558 op-amp gain stage with
//! **two 1N4148 diodes in series each way** in the feedback loop. Putting the
//! diodes in series doubles the ~0.6 V forward drop to ~1.2 V, so the clipper
//! only bites well up the swing — the pedal stays clean and touch-sensitive
//! far longer than a Tube Screamer. That headroom is the "uncompressed",
//! dynamic voice it is known for (the reputed "magic" 1963 Fender Deluxe
//! chime).
//!
//! What makes it its own voice in this family:
//! - **Full, un-scooped lows.** Unlike the ts9 (high-passed at 720 Hz *into*
//!   the gain, so only mids clip), just a gentle 70 Hz subsonic trim sits
//!   ahead of the clipper — a low note breaks up along with the mids. An
//!   amp-in-a-box, not a mid boost.
//! - **Fender chime.** A fixed bright pre-emphasis feeds sparkle into the
//!   clipper; the Treble knob is voiced open (2.8 kHz shelf) on top of it.
//! - **Mild bias asymmetry.** The stock 2+2 diode array is symmetric, but the
//!   Jan Ray's internal bias trim skews the operating point a touch. Modelled
//!   as slightly uneven knees (0.95 / 0.80), it adds the gentle even-harmonic
//!   warmth that reads as "tube-like" without tipping into the strong 2nd of
//!   the bd2/evva/red-charlie.
//!
//! The Timmy's signature trick — **Bass set *before* the clipper**, so the
//! low end stays tight when the gain is cranked — is idealized here: the fixed
//! pre-clip trim gives the tightness, and the faceplate **Bass** is a post-clip
//! low shelf. At the Jan Ray's low gain the two are nearly identical, and the
//! shelf is what the player actually hears. **Treble** is the post-clip shelf
//! (cut-only in hardware, centered to neutral here). Bass / Treble / Volume
//! over a four-knob Fender-style face — no mid control, by design.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 4] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("bass", "Bass", 5.0, 30.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("volume", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "jan-ray",
    name: "Jan Ray",
    params: &PARAMS,
};

/// Series-diode clip knees. Both are tall (≈0.6 V single-diode headroom
/// doubled) — that is the "uncompressed" reserve. The small gap between them
/// is the internal bias trim: a gentle even-harmonic warmth, far milder than
/// the evva's 0.8/0.5.
const KNEE_POS: f32 = 0.95;
const KNEE_NEG: f32 = 0.75;
/// Fixed bright pre-emphasis ahead of the clipper (+~2.6 dB above 2.2 kHz):
/// the Fender sparkle that feeds the top-end harmonics. In the angry-charlie's
/// neighborhood but voiced a touch smoother, so it chimes instead of fizzes.
const BRIGHT: f32 = 0.34;
/// Bright pre-emphasis corner.
const BRIGHT_HZ: f32 = 2_200.0;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.30;

/// Fender-style two-band tone corners: a low shelf (Bass) and a high shelf
/// (Treble). The 2.8 kHz treble corner is the Jan Ray's own (vs the Timmy's
/// brighter ~10 kHz).
const BASS_HZ: f32 = 120.0;
const TREBLE_HZ: f32 = 2_800.0;

pub(super) struct JanRay {
    hp70: OnePole,
    bright: OnePole,
    dc_os: OnePole,
    bass_lp: OnePole,
    treble_lp: OnePole,
    c70: f32,
    c_bright: f32,
    c12: f32,
    c_bass: f32,
    c_treble: f32,
}

impl JanRay {
    pub(super) fn new() -> Self {
        Self {
            hp70: OnePole::default(),
            bright: OnePole::default(),
            dc_os: OnePole::default(),
            bass_lp: OnePole::default(),
            treble_lp: OnePole::default(),
            c70: 0.0,
            c_bright: 0.0,
            c12: 0.0,
            c_bass: 0.0,
            c_treble: 0.0,
        }
    }
}

impl Circuit for JanRay {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        // Pre-clip trims and the DC blocker run at the oversampled rate; the
        // tone shelves are linear, at the base rate.
        self.c70 = lp_coeff(70.0, os_rate);
        self.c_bright = lp_coeff(BRIGHT_HZ, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.c_bass = lp_coeff(BASS_HZ, base_rate);
        self.c_treble = lp_coeff(TREBLE_HZ, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp70.reset();
        self.bright.reset();
        self.dc_os.reset();
        self.bass_lp.reset();
        self.treble_lp.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // +2 dB (a clean, transparent boost) to +30 dB (medium breakup) — a
        // low-gain pedal whose whole point is the clean end. Audio taper,
        // powf twice per chunk, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(2.0 + 28.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // Gentle 70 Hz subsonic trim — the lows otherwise stay full
            // (unlike the ts9's 720 Hz cut), just the flab tightened.
            let x = x - self.hp70.lp(x, self.c70);
            // Fixed bright pre-emphasis into the clipper: the chime.
            let x = x + BRIGHT * (x - self.bright.lp(x, self.c_bright));
            let v = gain.tick() * x;
            // Series-diode soft clip: tall, mildly asymmetric knees. The tall
            // threshold is the headroom that keeps it dynamic; the small
            // asymmetry is the bias-trim warmth.
            let clipped = if v >= 0.0 {
                KNEE_POS * (v / KNEE_POS).tanh()
            } else {
                KNEE_NEG * (v / KNEE_NEG).tanh()
            };
            *s = clipped - self.dc_os.lp(clipped, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No single tone knob — tone shaping lives in `eq`; `post` only
        // applies the output makeup.
        for s in block.iter_mut() {
            *s *= MAKEUP;
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], _mid: &[f32], high: &[f32]) {
        // Fender-style two-band tone: a 120 Hz low shelf (Bass) and a 2.8 kHz
        // high shelf (Treble). Knob 5 = flat (gain 0, a bit-for-bit no-op);
        // 0/10 = ∓12 dB. No mid band — by design. Gains ramped per chunk.
        let mut bass_gain = Ramp::over(low, |l| db_to_lin(-12.0 + 2.4 * l) - 1.0);
        let mut treble_gain = Ramp::over(high, |h| db_to_lin(-12.0 + 2.4 * h) - 1.0);
        for s in block.iter_mut() {
            let x = *s;
            let lo = self.bass_lp.lp(x, self.c_bass);
            let hi = x - self.treble_lp.lp(x, self.c_treble);
            *s = x + bass_gain.tick() * lo + treble_gain.tick() * hi;
        }
    }
}
