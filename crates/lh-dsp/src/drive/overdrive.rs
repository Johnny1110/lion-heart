//! **overdrive** — a smooth rational soft-clipper ported from DaisySP's
//! `Overdrive` (Émilie Gillet's Plaits waveshaper behind stmlib's `SoftClip`).
//! A single Drive knob schedules *both* the pre-gain into the clipper and a
//! matching post-gain that normalizes loudness, so the pedal stays roughly
//! level as it dirties up — the neat trick worth porting from DaisySP.
//!
//! DaisySP has no tone stack (it is a synth/DSP library, not a pedal), so the
//! Tone knob here is the family's usual one-pole dark↔bright tilt and Level is
//! the shared output law. `SoftClip` is an *odd* function, so — like the TS9's
//! matched feedback diodes — it makes only odd harmonics (no even-harmonic
//! "warmth"): a clean, symmetric overdrive.
//!
//! Origin: pichenettes/eurorack (MIT), ported to DaisySP by Electrosmith. The
//! knob→drive mapping and the tone/makeup stage are ours; the pre/post-gain
//! scheduling and the clip curve are DaisySP's, kept faithful.

use lh_core::{EffectDesc, ParamDesc};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "overdrive",
    name: "Overdrive",
    params: &PARAMS,
};

/// Output makeup. DaisySP's post-gain normalizes a full-scale modular signal;
/// fed a −18 dBFS guitar the clipper still runs hot at noon, so this trims the
/// stage back into the family's unity window (calibrated by
/// `modelled_pedals_sit_near_unity_at_default_knobs`).
const MAKEUP: f32 = 0.16;

/// Drive-knob ceiling into DaisySP's `drive_` mapping. DaisySP's loudness
/// correction rides a `drive·(2−drive)` window that collapses to zero exactly
/// at `drive = 1.0`, so the literal top of the pot would lose all makeup and
/// jump ~+10 dB over the setting just below it. Capping the top of the sweep a
/// hair short of 1.0 (only positions above ~9.5 are touched — the clip is
/// already at the square-wave ceiling there) keeps the auto-makeup working
/// across the *whole* knob, which is what DaisySP's design is actually after.
const DRIVE_CEILING: f32 = 0.95;

/// stmlib soft-limit: a cheap odd-symmetric rational approximation to `tanh`.
#[inline]
fn soft_limit(x: f32) -> f32 {
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// stmlib soft-clip: `soft_limit` inside ±3, hard ±1 beyond.
#[inline]
fn soft_clip(x: f32) -> f32 {
    if x < -3.0 {
        -1.0
    } else if x > 3.0 {
        1.0
    } else {
        soft_limit(x)
    }
}

pub(super) struct Overdrive {
    tone_lp: OnePole,
    dc: OnePole,
    c_tone: f32,
    c_dc: f32,
}

impl Overdrive {
    pub(super) fn new() -> Self {
        Self {
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c_tone: 0.0,
            c_dc: 0.0,
        }
    }

    /// DaisySP `SetDrive` pre-gain: a gentle linear ramp (`drive·0.5`) blended
    /// into a steep `drive⁵·24` term by `drive²`, so low knob positions boost
    /// mildly and the top of the sweep slams the clipper. `pos` is 0..10;
    /// DaisySP's internal `drive_` is `2·(pos/10)`.
    #[inline]
    fn pre_gain(pos: f32) -> f32 {
        let d = 2.0 * (pos * 0.1).min(DRIVE_CEILING);
        let d2 = d * d;
        let a = d * 0.5;
        let b = d2 * d2 * d * 24.0;
        a + (b - a) * d2
    }

    /// DaisySP `SetDrive` post-gain: the reciprocal of the clipper's response
    /// to a mid-level probe, so loudness barely moves as pre-gain climbs. The
    /// `drive·(2−drive)` window makes the correction strongest at noon (where
    /// pre-gain is already deep into the clip) and eases it toward the ends.
    #[inline]
    fn post_gain(pos: f32) -> f32 {
        let d = 2.0 * (pos * 0.1).min(DRIVE_CEILING);
        let squashed = d * (2.0 - d);
        let pre = Self::pre_gain(pos);
        1.0 / soft_clip(0.33 + squashed * (pre - 0.33))
    }
}

impl Circuit for Overdrive {
    fn prepare(&mut self, base_rate: f32, _os_rate: f32) {
        self.c_tone = lp_coeff(700.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // Pre- and post-gain depend on the drive knob only: map each twice per
        // chunk and ease per sample with the shared Ramp (the clip is the one
        // per-sample nonlinearity). `drive` is the oversampled-rate trajectory.
        let mut pre = Ramp::over(drive, Self::pre_gain);
        let mut post = Ramp::over(drive, Self::post_gain);
        for s in block.iter_mut() {
            *s = soft_clip(pre.tick() * *s) * post.tick();
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        for (s, t) in block.iter_mut().zip(tone) {
            let x = *s;
            let lp = self.tone_lp.lp(x, self.c_tone);
            let hp = x - lp;
            let n = t * 0.1;
            // 0 = dark (10% of the treble above 700 Hz), 10 = bright (+~5.6 dB).
            let bright = 0.1 + 1.8 * n * n;
            let y = (lp + bright * hp) * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}
