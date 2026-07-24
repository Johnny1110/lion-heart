//! **angry-charlie-v2** — the Angry Charlie pushed from Marshall crunch into
//! high-gain lead **distortion**, with a built-in **600–800 Hz midrange lift**
//! that makes it cut and sing. Same DNA as [`super::angry_charlie`] — red-LED
//! hard clipping outside the op-amp loop, full lows, a post-clip Marshall
//! Baxandall stack — but two changes give it its own voice:
//!
//! 1. A fixed **~700 Hz peaking boost ahead of the clipper** (the band a
//!    cranked Marshall lives in): it drives that midrange into the LEDs harder,
//!    so the pedal focuses and cuts the way a mid-boosted lead tone does,
//!    rather than the flatter response of the V1.
//! 2. **Two cascaded hard-clip stages** instead of one. The V1 rides clean up
//!    to the LED knee then slams flat once; V2 re-amplifies that clipped wave
//!    through a tightening interstage coupling into a second clamp, so the wall
//!    builds in layers with far more gain and sustain — a genuine distortion,
//!    not a crunch. A gentle post lowpass files the harshest fizz off the top.
//!
//! Symmetric LED clipping at both stages keeps the even harmonics starved (the
//! buzzsaw stack of odds), and the higher (than silicon) LED knee keeps it a
//! touch more open than the scooped `monster5150`; the fuller lows and the
//! 700 Hz focus are what set it apart from that metal voice.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, ToneStack, knob, lp_coeff};
use crate::blocks::biquad::Biquad;

static PARAMS: [ParamDesc; 5] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("bass", "Bass", 5.0, 30.0),
    knob("middle", "Middle", 5.0, 30.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("volume", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "angry-charlie-v2",
    name: "Angry Charlie V2",
    params: &PARAMS,
};

/// Symmetric hard-clip threshold: the red LEDs' forward voltage, shared by both
/// cascaded stages like matched pairs to ground — the angry-charlie identity.
const KNEE: f32 = 1.0;
/// Fixed second-stage gain (+~11 dB): re-amplifies the stage-1 clipped wave into
/// the final clamp. This second clip is what turns the crunch into distortion.
const STAGE2_GAIN: f32 = 3.5;
/// Fixed bright pre-emphasis ahead of the clipper (+~3 dB above 1.8 kHz): the
/// sizzle, carried over from the V1.
const BRIGHT: f32 = 0.35;
/// The built-in midrange lift: a peaking boost centered in the 600–800 Hz band
/// so the mids drive the clipper harder and the tone cuts.
const MID_BOOST_HZ: f32 = 700.0;
const MID_BOOST_DB: f32 = 6.0;
const MID_BOOST_Q: f32 = 2.0;
/// Post-distortion fizz lowpass — brighter than the monster5150's 6.8 kHz, this
/// is still a Marshall.
const FIZZ_HZ: f32 = 7_500.0;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.09;

/// Marshall-style Baxandall corners (shared with the V1).
const BASS_HZ: f32 = 90.0;
const MID_HZ: f32 = 500.0;
const TREBLE_HZ: f32 = 2_800.0;

pub(super) struct AngryCharlieV2 {
    hp_in: OnePole,
    mid_boost: Biquad,
    bright: OnePole,
    couple: OnePole,
    dc_os: OnePole,
    fizz: OnePole,
    stack: ToneStack,
    c_hp: f32,
    c1800: f32,
    c_couple: f32,
    c12: f32,
    c_fizz: f32,
}

impl AngryCharlieV2 {
    pub(super) fn new() -> Self {
        Self {
            hp_in: OnePole::default(),
            mid_boost: Biquad::default(),
            bright: OnePole::default(),
            couple: OnePole::default(),
            dc_os: OnePole::default(),
            fizz: OnePole::default(),
            stack: ToneStack::new(BASS_HZ, MID_HZ, TREBLE_HZ),
            c_hp: 0.0,
            c1800: 0.0,
            c_couple: 0.0,
            c12: 0.0,
            c_fizz: 0.0,
        }
    }
}

impl Circuit for AngryCharlieV2 {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_hp = lp_coeff(60.0, os_rate);
        self.c1800 = lp_coeff(1_800.0, os_rate);
        self.c_couple = lp_coeff(150.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.c_fizz = lp_coeff(FIZZ_HZ, base_rate);
        // The mid boost runs inside the oversampled shaper, ahead of the clip.
        self.mid_boost
            .set_peaking(os_rate, MID_BOOST_HZ, MID_BOOST_DB, MID_BOOST_Q);
        self.stack.prepare(base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp_in.reset();
        self.mid_boost.reset();
        self.bright.reset();
        self.couple.reset();
        self.dc_os.reset();
        self.fizz.reset();
        self.stack.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // Stage-1 gain: +14 dB (edge of breakup) to +64 dB (slamming the wall),
        // a hotter sweep than the V1's — the second stage does the rest. Audio
        // taper, powf twice per chunk, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(14.0 + 50.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // Tightening high-pass at 60 Hz: keeps the thick body, sheds the
            // sub-bass flab the higher gain would otherwise fart out.
            let x = x - self.hp_in.lp(x, self.c_hp);
            // The 700 Hz lift, then the fixed bright pre-emphasis, both feeding
            // the clipper.
            let x = self.mid_boost.process_sample(x);
            let x = x + BRIGHT * (x - self.bright.lp(x, self.c1800));
            // Stage 1: clean gain into a genuine LED hard clip.
            let s1 = (gain.tick() * x).clamp(-KNEE, KNEE);
            // Tighten between stages so the cascade stays articulate under gain.
            let s1 = s1 - self.couple.lp(s1, self.c_couple);
            // Stage 2: re-amplify the clipped wave and slam the wall again.
            let s2 = (STAGE2_GAIN * s1).clamp(-KNEE, KNEE);
            *s = s2 - self.dc_os.lp(s2, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No single tone knob — tone shaping lives in `eq`. `post` files the
        // fizz off the top and applies the output makeup.
        for s in block.iter_mut() {
            *s = self.fizz.lp(*s, self.c_fizz) * MAKEUP;
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        // Shared 3-band Baxandall stack, voiced at 90 Hz / 500 Hz / 2.8 kHz.
        self.stack.process(block, low, mid, high);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in boost really does lift the 600–800 Hz band: the peaking
    /// filter's response peaks inside the band and is up at least a few dB on
    /// its shoulders (250 Hz / 2 kHz). This pins the enhancement independently
    /// of the clipping.
    #[test]
    fn mid_boost_peaks_in_the_600_800_band() {
        let mut f = Biquad::default();
        f.set_peaking(192_000.0, MID_BOOST_HZ, MID_BOOST_DB, MID_BOOST_Q);
        let at = |hz: f32| f.magnitude_db(192_000.0, hz);
        let peak = at(700.0);
        assert!(
            peak > 4.0,
            "should boost ~{MID_BOOST_DB} dB at 700 Hz, got {peak:.1}"
        );
        assert!(at(600.0) > 2.0 && at(800.0) > 2.0, "band shoulders lifted");
        assert!(
            peak - at(250.0) > 3.0,
            "peak stands above the low-mid shoulder"
        );
        assert!(
            peak - at(2_000.0) > 3.0,
            "peak stands above the high-mid shoulder"
        );
    }
}
