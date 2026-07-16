//! Noise gate: peak envelope follower + hysteresis, the first pedal in a
//! high-gain chain. Attack is fast and fixed (1 ms) so pick transients pass;
//! release is the musical control.

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};

use crate::Effect;

static PARAMS: [ParamDesc; 2] = [
    ParamDesc {
        key: "threshold",
        name: "Threshold",
        unit: "dB",
        range: Range::Linear {
            min: -80.0,
            max: -20.0,
        },
        default: -50.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "release",
        name: "Release",
        unit: "ms",
        range: Range::Log {
            min: 10.0,
            max: 1_000.0,
        },
        default: 120.0,
        smoothing_ms: 0.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "gate",
    name: "Noise Gate",
    params: &PARAMS,
};

/// Hysteresis width: once open, the gate only closes when the envelope falls
/// this far below the open threshold, preventing chatter around the threshold.
const CLOSE_RATIO: f32 = 0.5; // -6 dB
const ATTACK_MS: f32 = 1.0;
const ENV_DECAY_MS: f32 = 20.0;

pub struct NoiseGate {
    sample_rate: u32,
    // control values
    thr_open: f32,
    release_ms: f32,
    // derived coefficients
    env_decay: f32,
    attack_coeff: f32,
    release_coeff: f32,
    // runtime state
    env: f32,
    gain: f32,
    open: bool,
}

impl Default for NoiseGate {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseGate {
    pub fn new() -> Self {
        let mut gate = Self {
            sample_rate: 48_000,
            thr_open: db_to_lin(PARAMS[0].default),
            release_ms: PARAMS[1].default,
            env_decay: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            env: 0.0,
            gain: 0.0,
            open: false,
        };
        gate.recompute();
        gate
    }

    fn recompute(&mut self) {
        self.env_decay = one_pole(ENV_DECAY_MS, self.sample_rate);
        self.attack_coeff = one_pole(ATTACK_MS, self.sample_rate);
        self.release_coeff = one_pole(self.release_ms, self.sample_rate);
    }
}

fn one_pole(ms: f32, sample_rate: u32) -> f32 {
    1.0 - (-1.0 / (ms * 1e-3 * sample_rate as f32)).exp()
}

impl Effect for NoiseGate {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.recompute();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 0.0;
        self.open = false;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => self.thr_open = db_to_lin(PARAMS[0].range.to_real(normalized)),
            1 => {
                self.release_ms = PARAMS[1].range.to_real(normalized);
                self.release_coeff = one_pole(self.release_ms, self.sample_rate);
            }
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let thr_close = self.thr_open * CLOSE_RATIO;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // Linked detector: the louder channel drives one shared gain, so
            // the stereo image never wobbles as the gate rides the edge.
            let a = l.abs().max(r.abs());
            // Peak follower: instant attack, exponential decay.
            self.env = if a > self.env {
                a
            } else {
                self.env + self.env_decay * (a - self.env)
            };
            self.open = if self.open {
                self.env >= thr_close
            } else {
                self.env >= self.thr_open
            };
            let (target, coeff) = if self.open {
                (1.0, self.attack_coeff)
            } else {
                (0.0, self.release_coeff)
            };
            self.gain += coeff * (target - self.gain);
            if !self.open && self.gain < 1e-6 {
                self.gain = 0.0; // fully closed; also kills denormal tails
            }
            *l *= self.gain;
            *r *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared() -> NoiseGate {
        let mut g = NoiseGate::new();
        g.prepare(SR);
        g
    }

    #[test]
    fn loud_signal_passes_after_attack() {
        let mut g = prepared();
        let mut x = sine(SR, 220.0, SR as usize / 2); // 0 dBFS, well above -50 dB
        let mut xr = x.clone();
        g.process(&mut x, &mut xr);
        assert_finite("gate output", &x);
        let tail = &x[SR as usize / 4..];
        assert!(
            rms(tail) > 0.6,
            "gate must be fully open, rms {}",
            rms(tail)
        );
    }

    #[test]
    fn quiet_signal_stays_gated() {
        let mut g = prepared();
        let quiet: Vec<f32> = sine(SR, 220.0, SR as usize / 2)
            .iter()
            .map(|s| s * db_to_lin(-70.0))
            .collect();
        let mut x = quiet.clone();
        let mut xr = x.clone();
        g.process(&mut x, &mut xr);
        let tail = &x[SR as usize / 4..];
        assert!(
            rms(tail) < rms(&quiet) * 0.05,
            "below-threshold signal must be attenuated"
        );
    }

    #[test]
    fn closes_within_release_window_after_signal_stops() {
        let mut g = prepared();
        let mut burst = sine(SR, 220.0, SR as usize / 10);
        let mut burst_r = burst.clone();
        g.process(&mut burst, &mut burst_r); // open the gate

        // 120 ms release + 20 ms envelope decay: settle well within 5×.
        let mut tail = sine(SR, 220.0, (SR as f32 * 0.8) as usize);
        for s in tail.iter_mut() {
            *s *= db_to_lin(-90.0); // essentially silence, but nonzero
        }
        let mut tail_r = tail.clone();
        g.process(&mut tail, &mut tail_r);
        let end = &tail[tail.len() - SR as usize / 10..];
        assert!(rms(end) < 1e-5, "gate must close, rms {}", rms(end));
    }

    #[test]
    fn silence_in_silence_out() {
        let mut g = prepared();
        let mut x = silence(4_096);
        let mut xr = silence(4_096);
        g.process(&mut x, &mut xr);
        assert!(peak_is_zero(&x) && peak_is_zero(&xr));
    }

    fn peak_is_zero(x: &[f32]) -> bool {
        x.iter().all(|s| *s == 0.0)
    }
}
