//! **ts9** — Tube-Screamer-style. The gained path is the input high-passed
//! at 720 Hz (4.7 kΩ + 47 nF into the op-amp), amplified 21–41 dB
//! (51 kΩ + 500 kΩ drive pot over 4.7 kΩ) and squashed by the feedback
//! diodes; summing it with the unity dry path is what makes the classic
//! mid-hump with lows that stay clean. The 51 pF feedback cap darkens the
//! clipped path as the drive pot rises. Tone is the usual one-pole
//! dark↔bright tilt around 723 Hz.

use lh_core::{EffectDesc, ParamDesc};

use super::{Circuit, OnePole, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "ts9",
    name: "TS9",
    params: &PARAMS,
};

/// Diode knee scale: the clipped path flattens out around ±0.65 of full
/// scale, leaving headroom for the dry sum.
const DIODE: f32 = 0.65;
/// Calibrated so drive 5 / tone 5 / level 6 lands near unity loudness
/// (asserted by `levels_roughly_matched`).
const MAKEUP: f32 = 0.22;

pub(super) struct Ts9 {
    hp720: OnePole,
    fb_lp: OnePole,
    tone_lp: OnePole,
    dc: OnePole,
    c720: f32,
    c723: f32,
    c_dc: f32,
    /// Feedback-cap lowpass coefficient, eased toward a per-chunk target.
    fb_c: f32,
    os_rate: f32,
}

impl Ts9 {
    pub(super) fn new() -> Self {
        Self {
            hp720: OnePole::default(),
            fb_lp: OnePole::default(),
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c720: 0.0,
            c723: 0.0,
            c_dc: 0.0,
            fb_c: 0.0,
            os_rate: 4.0 * 48_000.0,
        }
    }

    /// Feedback resistance for a drive-pot position 0..10 (51 kΩ series plus
    /// the 500 kΩ pot, audio taper).
    #[inline]
    fn feedback_ohms(pos: f32) -> f32 {
        let n = pos * 0.1;
        51_000.0 + 500_000.0 * n * n
    }
}

impl Circuit for Ts9 {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.os_rate = os_rate;
        self.c720 = lp_coeff(720.0, os_rate);
        self.c723 = lp_coeff(723.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        self.fb_c = lp_coeff(6_000.0, os_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp720.reset();
        self.fb_lp.reset();
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // The 51 pF across the feedback network: 5.7 kHz at max drive up to
        // far above Nyquist at min. One exp per chunk, eased per sample.
        let fc =
            1.0 / (std::f32::consts::TAU * Self::feedback_ohms(drive[drive.len() - 1]) * 51e-12);
        let fb_target = lp_coeff(fc.min(self.os_rate * 0.45), self.os_rate);
        for (s, d) in block.iter_mut().zip(drive) {
            self.fb_c += 0.002 * (fb_target - self.fb_c);
            let x = *s;
            let hp = x - self.hp720.lp(x, self.c720);
            let g = 1.0 + Self::feedback_ohms(*d) / 4_700.0;
            let v = g * hp / DIODE;
            let clipped = DIODE * (v / (1.0 + v * v).sqrt());
            // Unity dry plus the clipped mids: the screamer sum.
            *s = x + self.fb_lp.lp(clipped, self.fb_c);
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
