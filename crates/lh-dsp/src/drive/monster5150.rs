//! **monster5150** — EVH 5150-style high gain. The cascade goes one deeper
//! than the red-charlie: a warm input triode, a hot second stage, then a
//! *very* cold third clipper — the wall of saturation and singing sustain
//! come from stacking those knees, and even the gain floor is dirty (the
//! lead channel has no clean). What keeps the wall usable is the low end
//! being carved out *before* the gain: a hard input trim below ~120 Hz plus
//! a second, tighter interstage coupling at 180 Hz — palm mutes chug
//! instead of flubbing — while the Low knob adds lows back *after* the
//! distortion, the way the amp's resonance control does. A fixed bright
//! pre-emphasis keeps the sizzle at every gain, and a post-distortion
//! lowpass (~6.8 kHz) files the fizz off the top. Low/Mid/High reach
//! 80 Hz / 550 Hz / 3 kHz; Pre/Post Gain follow the amp's panel.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 5] = [
    knob("pre", "Pre Gain", 5.0, 20.0),
    knob("low", "Low", 5.0, 30.0),
    knob("mid", "Mid", 5.0, 30.0),
    knob("high", "High", 5.0, 30.0),
    knob("post", "Post Gain", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "monster5150",
    name: "Monster 5150",
    params: &PARAMS,
};

/// Stage-1 knees: warm input triode.
const STAGE1_KNEE_POS: f32 = 0.8;
const STAGE1_KNEE_NEG: f32 = 1.0;
/// Stage-2 knees: hot and moderately cold; opposite polarity models the
/// stage inversion.
const STAGE2_KNEE_POS: f32 = 1.0;
const STAGE2_KNEE_NEG: f32 = 0.55;
/// Fixed second-stage gain (+12 dB).
const STAGE2_GAIN: f32 = 4.0;
/// Stage-3 knees: the very cold clipper — colder than the red-charlie's —
/// shearing the (re-inverted) positive swing almost immediately.
const STAGE3_KNEE_POS: f32 = 0.35;
const STAGE3_KNEE_NEG: f32 = 0.9;
/// Fixed third-stage gain (+6 dB).
const STAGE3_GAIN: f32 = 2.0;
/// Fixed bright pre-emphasis into stage 1 (+~3 dB above 1.5 kHz): the 5150
/// sizzle rides at every gain, unlike the red-charlie's fading bright cap.
const BRIGHT: f32 = 0.4;
/// Input low trim below ~120 Hz — carved harder than the red-charlie's so
/// the deeper cascade never sees flub.
const LOW_TRIM: f32 = 0.65;
/// Post-distortion fizz lowpass corner.
const FIZZ_HZ: f32 = 6_800.0;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.25;

/// 3-band tone stack corner frequencies.
const BASS_HZ: f32 = 80.0;
const MID_HZ: f32 = 550.0;
const TREBLE_HZ: f32 = 3_000.0;

pub(super) struct Monster5150 {
    hp_in: OnePole,
    lf_trim: OnePole,
    bright: OnePole,
    couple1: OnePole,
    couple2: OnePole,
    dc_os: OnePole,
    fizz: OnePole,
    eq_lo: OnePole,
    /// Mid bandpass: cascaded one-poles for a peak at MID_HZ.
    eq_mid_lp: OnePole,
    eq_mid_hp: OnePole,
    eq_hi: OnePole,
    c40: f32,
    c120: f32,
    c1500: f32,
    c_couple1: f32,
    c_couple2: f32,
    c12: f32,
    c_fizz: f32,
    c_lo: f32,
    c_mid_wide: f32,
    c_mid_narrow: f32,
    c_hi: f32,
}

impl Monster5150 {
    pub(super) fn new() -> Self {
        Self {
            hp_in: OnePole::default(),
            lf_trim: OnePole::default(),
            bright: OnePole::default(),
            couple1: OnePole::default(),
            couple2: OnePole::default(),
            dc_os: OnePole::default(),
            fizz: OnePole::default(),
            eq_lo: OnePole::default(),
            eq_mid_lp: OnePole::default(),
            eq_mid_hp: OnePole::default(),
            eq_hi: OnePole::default(),
            c40: 0.0,
            c120: 0.0,
            c1500: 0.0,
            c_couple1: 0.0,
            c_couple2: 0.0,
            c12: 0.0,
            c_fizz: 0.0,
            c_lo: 0.0,
            c_mid_wide: 0.0,
            c_mid_narrow: 0.0,
            c_hi: 0.0,
        }
    }

    /// Asymmetric tanh clipper with independent knees per polarity.
    #[inline]
    fn clip(v: f32, knee_pos: f32, knee_neg: f32) -> f32 {
        if v >= 0.0 {
            knee_pos * (v / knee_pos).tanh()
        } else {
            knee_neg * (v / knee_neg).tanh()
        }
    }
}

impl Circuit for Monster5150 {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c40 = lp_coeff(40.0, os_rate);
        self.c120 = lp_coeff(120.0, os_rate);
        self.c1500 = lp_coeff(1_500.0, os_rate);
        self.c_couple1 = lp_coeff(120.0, os_rate);
        self.c_couple2 = lp_coeff(180.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.c_fizz = lp_coeff(FIZZ_HZ, base_rate);
        self.c_lo = lp_coeff(BASS_HZ, base_rate);
        // Bandpass: wide LP then HP via subtracting a narrower LP.
        self.c_mid_wide = lp_coeff(MID_HZ * 1.4, base_rate);
        self.c_mid_narrow = lp_coeff(MID_HZ / 1.4, base_rate);
        self.c_hi = lp_coeff(TREBLE_HZ, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp_in.reset();
        self.lf_trim.reset();
        self.bright.reset();
        self.couple1.reset();
        self.couple2.reset();
        self.dc_os.reset();
        self.fizz.reset();
        self.eq_lo.reset();
        self.eq_mid_lp.reset();
        self.eq_mid_hp.reset();
        self.eq_hi.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // Pre gain: +12 dB (already crunching) to +60 dB into the cascade —
        // there is no clean on this channel. Audio taper, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(12.0 + 48.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // Subsonic block at 40 Hz.
            let x = x - self.hp_in.lp(x, self.c40);
            // Carve the lows before any gain — the chug tightness.
            let x = x - LOW_TRIM * self.lf_trim.lp(x, self.c120);
            // Fixed bright pre-emphasis: the sizzle at every gain.
            let x = x + BRIGHT * (x - self.bright.lp(x, self.c1500));
            // Stage 1: warm asymmetric soft clip.
            let s1 = Self::clip(gain.tick() * x, STAGE1_KNEE_POS, STAGE1_KNEE_NEG);
            let s1 = s1 - self.couple1.lp(s1, self.c_couple1);
            // Stage 2: hot, moderately cold.
            let s2 = Self::clip(STAGE2_GAIN * s1, STAGE2_KNEE_POS, STAGE2_KNEE_NEG);
            // Tighter second coupling before the coldest stage.
            let s2 = s2 - self.couple2.lp(s2, self.c_couple2);
            // Stage 3: the very cold clipper — the wall.
            let s3 = Self::clip(STAGE3_GAIN * s2, STAGE3_KNEE_POS, STAGE3_KNEE_NEG);
            *s = s3 - self.dc_os.lp(s3, self.c12);
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
        // Post-distortion 3-band — the Low knob restores lows *after* the
        // clipping carved them (resonance-style), so tight and thick are not
        // in conflict:
        //
        //   low  — shelf at 80 Hz (±12 dB)
        //   mid  — bandpass centred at 550 Hz (±10 dB), Q ≈ 1.0
        //   high — shelf at 3 kHz (±12 dB)
        //
        // Knob 5 = flat (0 dB), 0/10 = cut/boost.
        for (s, (&l, (&m, &h))) in block.iter_mut().zip(low.iter().zip(mid.iter().zip(high))) {
            let x = *s;
            let lo = self.eq_lo.lp(x, self.c_lo);
            let hi = x - self.eq_hi.lp(x, self.c_hi);
            let bp_raw = self.eq_mid_lp.lp(x, self.c_mid_wide);
            let bp = bp_raw - self.eq_mid_hp.lp(bp_raw, self.c_mid_narrow);
            let lo_gain = db_to_lin(-12.0 + 2.4 * l);
            let mid_gain = db_to_lin(-10.0 + 2.0 * m);
            let hi_gain = db_to_lin(-12.0 + 2.4 * h);
            *s = x + (lo_gain - 1.0) * lo + (mid_gain - 1.0) * bp + (hi_gain - 1.0) * hi;
        }
    }
}
