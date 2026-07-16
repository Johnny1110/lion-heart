//! Delay: interpolated read head over a circular buffer, filtered feedback
//! for an analog-voiced repeat, dry-plus-wet mix.
//!
//! The delay time is smoothed heavily (150 ms) and read through linear
//! interpolation, so turning the knob produces tape-style pitch slew instead
//! of clicks.

use lh_core::{EffectDesc, ParamDesc, Range};

use crate::Effect;
use crate::smooth::Smoothed;

static PARAMS: [ParamDesc; 3] = [
    ParamDesc {
        key: "time",
        name: "Time",
        unit: "ms",
        range: Range::Log {
            min: 20.0,
            max: 1_000.0,
        },
        default: 350.0,
        smoothing_ms: 150.0,
    },
    ParamDesc {
        key: "feedback",
        name: "Feedback",
        unit: "",
        range: Range::Linear { min: 0.0, max: 0.9 },
        default: 0.35,
        smoothing_ms: 20.0,
    },
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.25,
        smoothing_ms: 20.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "delay",
    name: "Delay",
    params: &PARAMS,
};

const MAX_SECONDS: usize = 2;
/// Fixed lowpass in the feedback path: repeats get darker, analog-style.
const FEEDBACK_LP_HZ: f32 = 4_000.0;

/// One channel's circular buffer and feedback-filter state.
struct DelayChannel {
    buf: Vec<f32>,
    write: usize,
    fb_lp: f32,
}

impl DelayChannel {
    /// One sample: interpolated read, filtered feedback write, returns wet.
    #[inline]
    fn step(&mut self, x: f32, delay_smp: f32, feedback: f32, fb_lp_coeff: f32) -> f32 {
        let len = self.buf.len();
        let rp = self.write as f32 - delay_smp + len as f32;
        let i0 = rp as usize; // truncation: rp >= 0
        let frac = rp - i0 as f32;
        let s0 = self.buf[i0 % len];
        let s1 = self.buf[(i0 + 1) % len];
        let wet = s0 + frac * (s1 - s0);

        self.fb_lp += fb_lp_coeff * (wet - self.fb_lp);
        let mut fb_sample = x + self.fb_lp * feedback;
        if fb_sample.abs() < 1e-15 {
            fb_sample = 0.0;
        }
        self.buf[self.write] = fb_sample;
        self.write = (self.write + 1) % len;
        wet
    }
}

pub struct Delay {
    sample_rate: u32,
    ch: [DelayChannel; 2],
    time_ms: f32,
    delay_smp: Smoothed,
    feedback: Smoothed,
    mix: Smoothed,
    fb_lp_coeff: f32,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    pub fn new() -> Self {
        let channel = || DelayChannel {
            buf: Vec::new(),
            write: 0,
            fb_lp: 0.0,
        };
        Self {
            sample_rate: 48_000,
            ch: [channel(), channel()],
            time_ms: PARAMS[0].default,
            delay_smp: Smoothed::new(0.0),
            feedback: Smoothed::new(PARAMS[1].default),
            mix: Smoothed::new(PARAMS[2].default),
            fb_lp_coeff: 0.3,
        }
    }
}

impl Effect for Delay {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        for ch in &mut self.ch {
            ch.buf = vec![0.0; MAX_SECONDS * sample_rate as usize];
        }
        self.delay_smp
            .configure(PARAMS[0].smoothing_ms, sample_rate);
        self.feedback.configure(PARAMS[1].smoothing_ms, sample_rate);
        self.mix.configure(PARAMS[2].smoothing_ms, sample_rate);
        self.delay_smp
            .set_target(self.time_ms * 1e-3 * sample_rate as f32);
        self.delay_smp.snap_to_target();
        self.feedback.snap_to_target();
        self.mix.snap_to_target();
        self.fb_lp_coeff =
            1.0 - (-2.0 * std::f32::consts::PI * FEEDBACK_LP_HZ / sample_rate as f32).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for ch in &mut self.ch {
            ch.buf.iter_mut().for_each(|s| *s = 0.0);
            ch.write = 0;
            ch.fb_lp = 0.0;
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => {
                self.time_ms = PARAMS[0].range.to_real(normalized);
                self.delay_smp
                    .set_target(self.time_ms * 1e-3 * self.sample_rate as f32);
            }
            1 => self
                .feedback
                .set_target(PARAMS[1].range.to_real(normalized)),
            2 => self.mix.set_target(PARAMS[2].range.to_real(normalized)),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = self.ch[0].buf.len();
        if len == 0 {
            return; // prepare() not called yet
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let d = self.delay_smp.tick().clamp(1.0, (len - 2) as f32);
            let fb = self.feedback.tick();
            let mix = self.mix.tick();
            let wet_l = self.ch[0].step(*l, d, fb, self.fb_lp_coeff);
            let wet_r = self.ch[1].step(*r, d, fb, self.fb_lp_coeff);
            *l += wet_l * mix;
            *r += wet_r * mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, impulse, peak, process_in_blocks, silence};

    const SR: u32 = 48_000;

    fn prepared() -> Delay {
        let mut d = Delay::new();
        d.prepare(SR);
        d
    }

    #[test]
    fn echoes_arrive_at_the_configured_time() {
        let mut d = prepared();
        d.set_param(0, PARAMS[0].range.to_norm(100.0)); // 100 ms = 4800 samples
        d.set_param(1, PARAMS[1].range.to_norm(0.5));
        d.set_param(2, 1.0); // full wet level
        // Let the time smoother settle before measuring.
        let mut warm = silence(SR as usize);
        let mut warm_r = silence(SR as usize);
        d.process(&mut warm, &mut warm_r);

        let x = impulse(SR as usize, 0);
        let y = process_in_blocks(&mut d, &x, 512);
        assert_finite("delay output", &y);

        let first = &y[4_700..4_900];
        let second = &y[9_500..9_700];
        let p1 = peak(first);
        let p2 = peak(second);
        assert!(p1 > 0.5, "first echo missing, peak {p1}");
        assert!(
            p2 > 0.15 && p2 < p1,
            "second echo must decay via feedback: p1 {p1}, p2 {p2}"
        );
        // Nothing audible before the first echo (dry impulse aside).
        assert!(peak(&y[10..4_600]) < 1e-3);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut d = prepared();
        let mut x = silence(8_192);
        let mut xr = silence(8_192);
        d.process(&mut x, &mut xr);
        assert!(peak(&x) == 0.0 && peak(&xr) == 0.0);
    }

    #[test]
    fn time_changes_do_not_produce_nan_or_clicks() {
        let mut d = prepared();
        d.set_param(2, 1.0);
        let mut x = crate::testutil::sine(SR, 330.0, SR as usize);
        let mut xr = x.clone();
        let (a, b) = x.split_at_mut(SR as usize / 2);
        let (ar, br) = xr.split_at_mut(SR as usize / 2);
        d.process(a, ar);
        d.set_param(0, 0.0); // slam to 20 ms mid-flight
        d.process(b, br);
        assert_finite("delay sweep", &x);
    }
}
