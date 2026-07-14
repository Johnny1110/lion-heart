//! Drive: asymmetric tanh waveshaper run at 4× oversampling, followed by a
//! DC blocker (the asymmetry generates signal-dependent DC), a one-pole tone
//! lowpass, and an output level.
//!
//! Signal path: input gain → ↑4× → shape → ↓4× → DC block → tone → level.

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};

use crate::Effect;
use crate::oversample::Oversampler4x;
use crate::smooth::Smoothed;

static PARAMS: [ParamDesc; 3] = [
    ParamDesc {
        key: "drive",
        name: "Drive",
        unit: "dB",
        range: Range::Linear {
            min: 0.0,
            max: 40.0,
        },
        default: 16.0,
        smoothing_ms: 20.0,
    },
    ParamDesc {
        key: "tone",
        name: "Tone",
        unit: "Hz",
        range: Range::Log {
            min: 500.0,
            max: 8_000.0,
        },
        default: 3_200.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "level",
        name: "Level",
        unit: "dB",
        range: Range::Linear {
            min: -24.0,
            max: 6.0,
        },
        default: -6.0,
        smoothing_ms: 20.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "drive",
    name: "Drive",
    params: &PARAMS,
};

/// Static bias into the tanh: breaks symmetry so even harmonics appear
/// (tube-flavoured). The constant output offset is removed analytically and
/// the signal-dependent remainder by the DC blocker.
const BIAS: f32 = 0.2;
const DC_R: f32 = 0.995;

pub struct Drive {
    sample_rate: u32,
    os: Oversampler4x,
    pregain: Smoothed,
    tone_coeff: Smoothed,
    level: Smoothed,
    tone_hz: f32,
    // filter memories
    dc_x1: f32,
    dc_y1: f32,
    lp: f32,
}

impl Default for Drive {
    fn default() -> Self {
        Self::new()
    }
}

impl Drive {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000,
            os: Oversampler4x::new(),
            pregain: Smoothed::new(db_to_lin(PARAMS[0].default)),
            tone_coeff: Smoothed::new(0.5),
            level: Smoothed::new(db_to_lin(PARAMS[2].default)),
            tone_hz: PARAMS[1].default,
            dc_x1: 0.0,
            dc_y1: 0.0,
            lp: 0.0,
        }
    }
}

fn lp_coeff(hz: f32, sample_rate: u32) -> f32 {
    1.0 - (-2.0 * std::f32::consts::PI * hz / sample_rate as f32).exp()
}

/// tanh(BIAS), precomputed (rustc has no const tanh).
const BIAS_TANH: f32 = 0.197_375_32;

#[inline]
fn shape(x: f32) -> f32 {
    // Subtracting the constant tanh(BIAS) recenters the idle point at zero.
    (x + BIAS).tanh() - BIAS_TANH
}

impl Effect for Drive {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.pregain.configure(PARAMS[0].smoothing_ms, sample_rate);
        self.tone_coeff
            .configure(PARAMS[1].smoothing_ms, sample_rate);
        self.level.configure(PARAMS[2].smoothing_ms, sample_rate);
        self.tone_coeff
            .set_target(lp_coeff(self.tone_hz, sample_rate));
        self.pregain.snap_to_target();
        self.tone_coeff.snap_to_target();
        self.level.snap_to_target();
        self.reset();
    }

    fn reset(&mut self) {
        self.os.reset();
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
        self.lp = 0.0;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => self
                .pregain
                .set_target(db_to_lin(PARAMS[0].range.to_real(normalized))),
            1 => {
                self.tone_hz = PARAMS[1].range.to_real(normalized);
                self.tone_coeff
                    .set_target(lp_coeff(self.tone_hz, self.sample_rate));
            }
            2 => self
                .level
                .set_target(db_to_lin(PARAMS[2].range.to_real(normalized))),
            _ => {}
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        // Input gain at the base rate (scalar gain commutes with resampling).
        for x in block.iter_mut() {
            *x *= self.pregain.tick();
        }
        self.os.process(block, |os_block| {
            for s in os_block.iter_mut() {
                *s = shape(*s);
            }
        });
        for x in block.iter_mut() {
            // DC blocker: y[n] = x[n] - x[n-1] + R·y[n-1]
            let y = *x - self.dc_x1 + DC_R * self.dc_y1;
            self.dc_x1 = *x;
            self.dc_y1 = if y.abs() < 1e-15 { 0.0 } else { y };
            // Tone: one-pole lowpass with a smoothed coefficient.
            let c = self.tone_coeff.tick();
            self.lp += c * (self.dc_y1 - self.lp);
            *x = self.lp * self.level.tick();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, process_in_blocks, sine};

    const SR: u32 = 48_000;

    fn prepared() -> Drive {
        let mut d = Drive::new();
        d.prepare(SR);
        d
    }

    #[test]
    fn output_is_finite_and_bounded_at_max_drive() {
        let mut d = prepared();
        d.set_param(0, 1.0); // 40 dB
        d.set_param(2, 1.0); // +6 dB
        let x = sine(SR, 220.0, SR as usize / 2);
        let y = process_in_blocks(&mut d, &x, 64);
        assert_finite("drive output", &y);
        let p = crate::testutil::peak(&y);
        assert!(p < 3.0, "bounded output, got peak {p}");
        assert!(p > 0.5, "signal present, got peak {p}");
    }

    #[test]
    fn asymmetric_shaper_leaves_no_dc() {
        let mut d = prepared();
        d.set_param(0, 1.0);
        let x = sine(SR, 220.0, SR as usize);
        let y = process_in_blocks(&mut d, &x, 256);
        let tail = &y[SR as usize / 2..];
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 1e-3, "DC blocker must remove offset: {mean}");
    }

    #[test]
    fn creates_harmonics() {
        let mut d = prepared();
        let f0 = 220.0;
        let x = sine(SR, f0, SR as usize);
        let y = process_in_blocks(&mut d, &x, 256);
        let tail = &y[SR as usize / 2..];

        // Project onto the fundamental (sin & cos at f0); what remains is
        // harmonic content created by the shaper.
        let n = tail.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in tail.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(f0) * i as f64 / f64::from(SR);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        let fund_rms = ((cs * 2.0 / n).powi(2) + (cc * 2.0 / n).powi(2)).sqrt() / 2f64.sqrt();
        let total_rms = f64::from(crate::testutil::rms(tail));
        let residual = (total_rms.powi(2) - fund_rms.powi(2)).max(0.0).sqrt();
        assert!(
            residual / total_rms > 0.05,
            "expected >5% harmonic content, got {:.3}",
            residual / total_rms
        );
    }

    #[test]
    fn param_changes_are_smooth() {
        let mut d = prepared();
        d.set_param(0, 0.0);
        let x = sine(SR, 220.0, SR as usize / 2);
        let mut y = x.clone();
        let (a, b) = y.split_at_mut(SR as usize / 4);
        d.process(a);
        d.set_param(0, 1.0); // slam the knob mid-stream
        d.set_param(2, 0.0);
        d.process(b);
        assert_finite("drive sweep", &y);
        let max_step = y
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        // 220 Hz at unity swings ~0.03/sample; a hard gain jump would spike this.
        assert!(max_step < 0.5, "click detected: step {max_step}");
    }
}
