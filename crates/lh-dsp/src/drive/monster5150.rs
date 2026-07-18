//! **monster5150** — 80s-metal-mod high gain. The tube-warm tanh cascade is
//! gone; every stage now clips with a genuine **symmetric hard clamp** — the
//! diode-to-ground clipping mod period thrash/speed players bolted onto
//! their Marshalls and Boogies for more sustain and a harsher, buzzsaw edge
//! than the amp's own tubes gave on their own. Three cascaded clips (the
//! same threshold each time, like matched diode pairs) still go one deeper
//! than the red-charlie's two soft ones, so the wall builds in layers
//! instead of one flat ceiling; the low end is carved out *before* any of
//! that clipping — a hard input trim below ~120 Hz plus a second, tighter
//! interstage coupling at 180 Hz — so palm mutes chug instead of flubbing
//! out, and even the gain floor is dirty (the lead channel has no clean).
//! The Low knob adds lows back *after* the distortion (resonance-style, the
//! way the amp's own control works), a fixed bright pre-emphasis keeps the
//! sizzle at every gain, and a post-distortion lowpass (~6.8 kHz) files the
//! harshest fizz off the top. Symmetric clipping throughout starves the even
//! harmonics, leaving the buzzsaw stack of odds that made those hot-rodded
//! 80s rigs sound so vicious. Low/Mid/High reach 80 Hz / 550 Hz / 3 kHz;
//! Pre/Post Gain follow the amp's panel.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, ToneStack, knob, lp_coeff};

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

/// Shared symmetric hard-clip ceiling — every stage clamps at the same
/// threshold, like matched diode pairs to ground; three of them cascade
/// (with interstage filtering reshaping the wave between clips) instead of
/// one softer knee doing all the work.
const KNEE: f32 = 0.85;
/// Fixed second-stage gain (+12 dB) driving the already-clipped signal back
/// into the next clamp.
const STAGE2_GAIN: f32 = 4.0;
/// Fixed third-stage gain (+6 dB) into the final, tightest wall.
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
const MAKEUP: f32 = 0.16;

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
    stack: ToneStack,
    c40: f32,
    c120: f32,
    c1500: f32,
    c_couple1: f32,
    c_couple2: f32,
    c12: f32,
    c_fizz: f32,
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
            stack: ToneStack::new(BASS_HZ, MID_HZ, TREBLE_HZ),
            c40: 0.0,
            c120: 0.0,
            c1500: 0.0,
            c_couple1: 0.0,
            c_couple2: 0.0,
            c12: 0.0,
            c_fizz: 0.0,
        }
    }

    /// Symmetric hard clip — a genuine flat-topped clamp, not a tanh curve:
    /// the diode-to-ground character, both polarities alike.
    #[inline]
    fn hard_clip(v: f32) -> f32 {
        v.clamp(-KNEE, KNEE)
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
        self.stack.prepare(base_rate);
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
        self.stack.reset();
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
            // Stage 1: symmetric hard clip to the shared ceiling.
            let s1 = Self::hard_clip(gain.tick() * x);
            let s1 = s1 - self.couple1.lp(s1, self.c_couple1);
            // Stage 2: re-amplify the already-clipped wave and clamp again.
            let s2 = Self::hard_clip(STAGE2_GAIN * s1);
            // Tighter second coupling before the final stage.
            let s2 = s2 - self.couple2.lp(s2, self.c_couple2);
            // Stage 3: the last, tightest slam of the wall.
            let s3 = Self::hard_clip(STAGE3_GAIN * s2);
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
        // Shared 3-band stack, voiced at 80 Hz / 550 Hz / 3 kHz. Post-
        // distortion placement is the point: the Low knob restores lows
        // *after* the clipping carved them (resonance-style), so tight and
        // thick are not in conflict.
        self.stack.process(block, low, mid, high);
    }
}
