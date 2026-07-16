//! The modulation family: one pedal, four voices — chorus, flanger, phaser,
//! tremolo — selected by a stepped `type` parameter (white paper puts a
//! single `mod` position in the chain). All share one LFO and the
//! rate/depth/feedback/mix controls:
//!
//! - **chorus**: delay line swept 2–14 ms, gentle feedback
//! - **flanger**: delay line swept 1–5 ms, prominent feedback
//! - **phaser**: four first-order allpass stages, cutoff swept 230–2100 Hz
//! - **tremolo**: amplitude LFO (feedback unused)
//!
//! Switching type resets the voice state (a brief discontinuity while
//! auditioning types, never NaN); continuous params morph smoothly.

use lh_core::{EffectDesc, ParamDesc, Range};

use crate::Effect;
use crate::smooth::Smoothed;

pub static TYPES: [&str; 4] = ["chorus", "flanger", "phaser", "tremolo"];

const CHORUS: usize = 0;
const FLANGER: usize = 1;
const PHASER: usize = 2;
const TREMOLO: usize = 3;

/// Longest modulated delay (chorus max) plus headroom.
const MAX_DELAY_MS: f32 = 20.0;
const PHASER_STAGES: usize = 4;

static PARAMS: [ParamDesc; 5] = [
    ParamDesc {
        key: "type",
        name: "Type",
        unit: "",
        range: Range::Stepped { labels: &TYPES },
        default: 0.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "rate",
        name: "Rate",
        unit: "Hz",
        range: Range::Log {
            min: 0.05,
            max: 10.0,
        },
        default: 0.8,
        smoothing_ms: 80.0,
    },
    ParamDesc {
        key: "depth",
        name: "Depth",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.5,
        smoothing_ms: 50.0,
    },
    ParamDesc {
        key: "feedback",
        name: "Feedback",
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 0.85,
        },
        default: 0.25,
        smoothing_ms: 50.0,
    },
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.5,
        smoothing_ms: 30.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "mod",
    name: "Modulation",
    params: &PARAMS,
};

pub struct Modulation {
    sample_rate: f32,
    mode: usize,
    rate: Smoothed,
    depth: Smoothed,
    feedback: Smoothed,
    mix: Smoothed,
    phase: f32,
    // chorus / flanger voice
    buf: Vec<f32>,
    write: usize,
    // phaser voice: per-stage previous input / output
    ap_x1: [f32; PHASER_STAGES],
    ap_y1: [f32; PHASER_STAGES],
    // shared feedback memory (last wet sample)
    fb: f32,
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulation {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            mode: PARAMS[0].default as usize,
            rate: Smoothed::new(PARAMS[1].default),
            depth: Smoothed::new(PARAMS[2].default),
            feedback: Smoothed::new(PARAMS[3].default),
            mix: Smoothed::new(PARAMS[4].default),
            phase: 0.0,
            buf: Vec::new(),
            write: 0,
            ap_x1: [0.0; PHASER_STAGES],
            ap_y1: [0.0; PHASER_STAGES],
            fb: 0.0,
        }
    }

    fn clear_voice(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.ap_x1 = [0.0; PHASER_STAGES];
        self.ap_y1 = [0.0; PHASER_STAGES];
        self.fb = 0.0;
    }

    /// Interpolated read `delay_smp` samples behind the write head.
    #[inline]
    fn read_delayed(&self, delay_smp: f32) -> f32 {
        let len = self.buf.len() as f32;
        let rp = self.write as f32 - delay_smp + len;
        let i0 = rp as usize;
        let frac = rp - i0 as f32;
        let a = self.buf[i0 % self.buf.len()];
        let b = self.buf[(i0 + 1) % self.buf.len()];
        a + frac * (b - a)
    }
}

impl Effect for Modulation {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        self.buf = vec![0.0; (MAX_DELAY_MS * 1e-3 * self.sample_rate) as usize + 4];
        for (smoothed, desc) in [
            (&mut self.rate, &PARAMS[1]),
            (&mut self.depth, &PARAMS[2]),
            (&mut self.feedback, &PARAMS[3]),
            (&mut self.mix, &PARAMS[4]),
        ] {
            smoothed.configure(desc.smoothing_ms, sample_rate);
            smoothed.snap_to_target();
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.clear_voice();
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let real = PARAMS[index].range.to_real(normalized);
        match index {
            0 => {
                let mode = real as usize;
                if mode != self.mode {
                    self.mode = mode;
                    self.clear_voice();
                }
            }
            1 => self.rate.set_target(real),
            2 => self.depth.set_target(real),
            3 => self.feedback.set_target(real),
            4 => self.mix.set_target(real),
            _ => {}
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        if self.buf.is_empty() {
            return; // prepare() not called yet
        }
        let ms = self.sample_rate * 1e-3;
        for x in block.iter_mut() {
            let rate = self.rate.tick();
            let depth = self.depth.tick();
            let feedback = self.feedback.tick();
            let mix = self.mix.tick();

            self.phase += std::f32::consts::TAU * rate / self.sample_rate;
            if self.phase >= std::f32::consts::TAU {
                self.phase -= std::f32::consts::TAU;
            }
            let lfo = self.phase.sin(); // -1..1

            let dry = *x;
            let wet = match self.mode {
                CHORUS | FLANGER => {
                    let delay_ms = if self.mode == CHORUS {
                        8.0 + 6.0 * depth * lfo // 2..14 ms
                    } else {
                        1.0 + 4.0 * depth * (0.5 + 0.5 * lfo) // 1..5 ms
                    };
                    let delay_smp = (delay_ms * ms).clamp(1.0, (self.buf.len() - 2) as f32);
                    let tap = self.read_delayed(delay_smp);
                    self.buf[self.write] = dry + feedback * tap;
                    self.write = (self.write + 1) % self.buf.len();
                    tap
                }
                PHASER => {
                    // Sweep the allpass corner geometrically around 700 Hz.
                    let fc = 700.0 * 3f32.powf(lfo * depth);
                    let t = (std::f32::consts::PI * fc / self.sample_rate).tan();
                    let a = (1.0 - t) / (1.0 + t);
                    let mut y = dry + feedback * self.fb;
                    for stage in 0..PHASER_STAGES {
                        let out = -a * y + self.ap_x1[stage] + a * self.ap_y1[stage];
                        self.ap_x1[stage] = y;
                        self.ap_y1[stage] = out;
                        y = out;
                    }
                    self.fb = y;
                    y
                }
                TREMOLO => dry * (1.0 - depth * (0.5 + 0.5 * lfo)),
                _ => dry, // unreachable: stepped range clamps to 0..=3
            };

            *x = dry + mix * (wet - dry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared(mode: usize) -> Modulation {
        let mut m = Modulation::new();
        m.prepare(SR);
        m.set_param(0, PARAMS[0].range.to_norm(mode as f32));
        m
    }

    fn set(m: &mut Modulation, index: usize, real: f32) {
        m.set_param(index, PARAMS[index].range.to_norm(real));
    }

    #[test]
    fn all_modes_render_finite_bounded_audio() {
        for (mode, name) in TYPES.iter().enumerate() {
            let mut m = prepared(mode);
            set(&mut m, 3, 0.85); // max feedback
            let mut x = sine(SR, 220.0, SR as usize);
            for block in x.chunks_mut(64) {
                m.process(block);
            }
            assert_finite(name, &x);
            let peak = x.iter().fold(0.0f32, |p, s| p.max(s.abs()));
            assert!(peak < 4.0, "{name} runs away: peak {peak}");
        }
    }

    #[test]
    fn mix_zero_is_bit_exact_dry() {
        for (mode, name) in TYPES.iter().enumerate() {
            let mut m = prepared(mode);
            set(&mut m, 4, 0.0);
            // Let the 30 ms mix smoothing decay all the way to the snap
            // threshold (~20 time constants) before comparing.
            let mut warm = sine(SR, 220.0, SR as usize);
            m.process(&mut warm);
            let x = sine(SR, 220.0, 8_192);
            let mut y = x.clone();
            m.process(&mut y);
            assert_eq!(x, y, "{name} must pass dry at mix 0");
        }
    }

    #[test]
    fn output_is_time_varying() {
        // The same input block must not produce the same output twice in a
        // row — the LFO has moved. (Tremolo included: 4 Hz over 100 ms.)
        for (mode, name) in TYPES.iter().enumerate() {
            let mut m = prepared(mode);
            set(&mut m, 1, 4.0);
            set(&mut m, 2, 1.0);
            set(&mut m, 4, 1.0);
            let x = sine(SR, 220.0, 4_800);
            let mut first = x.clone();
            m.process(&mut first);
            let mut second = x.clone();
            m.process(&mut second);
            assert_ne!(first, second, "{name} must modulate over time");
        }
    }

    #[test]
    fn tremolo_pumps_at_the_lfo_rate() {
        let mut m = prepared(TREMOLO);
        set(&mut m, 1, 4.0);
        set(&mut m, 2, 1.0);
        set(&mut m, 4, 1.0);
        let mut x = sine(SR, 220.0, SR as usize); // 1 s = 4 LFO cycles
        m.process(&mut x);
        // 25 ms windows: min RMS must dip far below max RMS.
        let win = SR as usize / 40;
        let rms_per: Vec<f32> = x[SR as usize / 2..].chunks(win).map(rms).collect();
        let max = rms_per.iter().fold(0.0f32, |m, v| m.max(*v));
        let min = rms_per.iter().fold(f32::INFINITY, |m, v| m.min(*v));
        assert!(min < 0.4 * max, "tremolo must pump: min {min}, max {max}");
    }

    #[test]
    fn type_switch_mid_stream_stays_finite() {
        let mut m = prepared(CHORUS);
        set(&mut m, 3, 0.85);
        let mut x = sine(SR, 220.0, SR as usize / 4);
        m.process(&mut x);
        for mode in [FLANGER, PHASER, TREMOLO, CHORUS] {
            m.set_param(0, PARAMS[0].range.to_norm(mode as f32));
            let mut y = sine(SR, 220.0, SR as usize / 4);
            m.process(&mut y);
            assert_finite("after type switch", &y);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for (mode, name) in TYPES.iter().enumerate() {
            let mut m = prepared(mode);
            let mut x = silence(8_192);
            m.process(&mut x);
            assert!(rms(&x) == 0.0, "{name} must stay silent");
        }
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            for mode in 0..TYPES.len() {
                let mut m = Modulation::new();
                m.prepare(sr);
                m.set_param(0, PARAMS[0].range.to_norm(mode as f32));
                for chunk in [32usize, 483, 1_024] {
                    let mut x = sine(sr, 440.0, 4_096);
                    for block in x.chunks_mut(chunk) {
                        m.process(block);
                    }
                    assert_finite("mod multirate", &x);
                }
            }
        }
    }
}
