//! Output safety limiter (white paper §3.3): the last thing before the DAC.
//! A fast peak limiter with a hard ceiling — no patch, feedback setting, or
//! bug may slam the monitors (or ears).

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};

use crate::Effect;

static PARAMS: [ParamDesc; 2] = [
    ParamDesc {
        key: "ceiling",
        name: "Ceiling",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 0.0,
        },
        default: -1.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "release",
        name: "Release",
        unit: "ms",
        range: Range::Log {
            min: 10.0,
            max: 500.0,
        },
        default: 60.0,
        smoothing_ms: 0.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "limiter",
    name: "Limiter",
    params: &PARAMS,
};

pub struct Limiter {
    sample_rate: u32,
    ceiling: f32,
    release_ms: f32,
    release_coeff: f32,
    /// Current gain reduction state (1.0 = no reduction).
    gain: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        let mut l = Self {
            sample_rate: 48_000,
            ceiling: db_to_lin(PARAMS[0].default),
            release_ms: PARAMS[1].default,
            release_coeff: 0.0,
            gain: 1.0,
        };
        l.recompute();
        l
    }

    fn recompute(&mut self) {
        self.release_coeff =
            1.0 - (-1.0 / (self.release_ms * 1e-3 * self.sample_rate as f32)).exp();
    }
}

impl Effect for Limiter {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.recompute();
        self.reset();
    }

    fn reset(&mut self) {
        self.gain = 1.0;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => self.ceiling = db_to_lin(PARAMS[0].range.to_real(normalized)),
            1 => {
                self.release_ms = PARAMS[1].range.to_real(normalized);
                self.recompute();
            }
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // Linked: the louder channel sets one shared reduction.
            let a = l.abs().max(r.abs());
            // Instant attack: jump straight to the required reduction;
            // smooth release back toward unity.
            let need = if a * self.gain > self.ceiling {
                self.ceiling / a.max(1e-9)
            } else {
                1.0
            };
            self.gain = if need < self.gain {
                need
            } else {
                self.gain + self.release_coeff * (need - self.gain)
            };
            // Hard ceiling as the absolute guarantee.
            *l = (*l * self.gain).clamp(-self.ceiling, self.ceiling);
            *r = (*r * self.gain).clamp(-self.ceiling, self.ceiling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, sine};

    const SR: u32 = 48_000;

    #[test]
    fn quiet_signals_pass_untouched() {
        let mut l = Limiter::new();
        l.prepare(SR);
        let x: Vec<f32> = sine(SR, 220.0, 4_096).iter().map(|s| s * 0.25).collect();
        let mut y = x.clone();
        let mut yr = x.clone();
        l.process(&mut y, &mut yr);
        let max_err = x
            .iter()
            .zip(&y)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-6, "below ceiling must be unity, err {max_err}");
    }

    #[test]
    fn hot_signals_never_exceed_the_ceiling() {
        let mut l = Limiter::new();
        l.prepare(SR);
        l.set_param(0, PARAMS[0].range.to_norm(-6.0));
        let mut y: Vec<f32> = sine(SR, 220.0, SR as usize / 2)
            .iter()
            .map(|s| s * 2.0) // +6 dB over full scale
            .collect();
        let mut yr = y.clone();
        l.process(&mut y, &mut yr);
        assert_finite("limiter output", &y);
        let ceiling = db_to_lin(-6.0);
        assert!(
            peak(&y) <= ceiling + 1e-4,
            "peak {} exceeds ceiling {}",
            peak(&y),
            ceiling
        );
        // Still audible, not muted.
        assert!(peak(&y[SR as usize / 4..]) > ceiling * 0.8);
    }
}
