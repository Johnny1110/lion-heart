//! Compressor: feed-forward peak compressor, second pedal in the chain.
//! A branching envelope follower (attack when rising, release when falling)
//! drives a dB-domain gain computer — the classic pedal topology, so attack
//! and release behave the way guitarists expect.

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin, lin_to_db};

use crate::Effect;
use crate::smooth::Smoothed;

static PARAMS: [ParamDesc; 5] = [
    ParamDesc {
        key: "threshold",
        name: "Threshold",
        unit: "dB",
        range: Range::Linear {
            min: -60.0,
            max: 0.0,
        },
        default: -24.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "ratio",
        name: "Ratio",
        unit: ":1",
        range: Range::Linear {
            min: 1.0,
            max: 20.0,
        },
        default: 4.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "attack",
        name: "Attack",
        unit: "ms",
        range: Range::Log {
            min: 0.1,
            max: 100.0,
        },
        default: 5.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "release",
        name: "Release",
        unit: "ms",
        range: Range::Log {
            min: 20.0,
            max: 1_000.0,
        },
        default: 120.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "makeup",
        name: "Makeup",
        unit: "dB",
        range: Range::Linear {
            min: 0.0,
            max: 24.0,
        },
        default: 0.0,
        smoothing_ms: 30.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "comp",
    name: "Compressor",
    params: &PARAMS,
};

pub struct Compressor {
    sample_rate: u32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup: Smoothed,
    attack_coeff: f32,
    release_coeff: f32,
    env: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        let mut comp = Self {
            sample_rate: 48_000,
            threshold_db: PARAMS[0].default,
            ratio: PARAMS[1].default,
            attack_ms: PARAMS[2].default,
            release_ms: PARAMS[3].default,
            makeup: Smoothed::new(db_to_lin(PARAMS[4].default)),
            attack_coeff: 0.0,
            release_coeff: 0.0,
            env: 0.0,
        };
        comp.recompute();
        comp
    }

    fn recompute(&mut self) {
        self.attack_coeff = one_pole(self.attack_ms, self.sample_rate);
        self.release_coeff = one_pole(self.release_ms, self.sample_rate);
    }
}

fn one_pole(ms: f32, sample_rate: u32) -> f32 {
    1.0 - (-1.0 / (ms * 1e-3 * sample_rate as f32)).exp()
}

impl Effect for Compressor {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.recompute();
        self.makeup.configure(PARAMS[4].smoothing_ms, sample_rate);
        self.makeup.snap_to_target();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => self.threshold_db = PARAMS[0].range.to_real(normalized),
            1 => self.ratio = PARAMS[1].range.to_real(normalized),
            2 => {
                self.attack_ms = PARAMS[2].range.to_real(normalized);
                self.attack_coeff = one_pole(self.attack_ms, self.sample_rate);
            }
            3 => {
                self.release_ms = PARAMS[3].range.to_real(normalized);
                self.release_coeff = one_pole(self.release_ms, self.sample_rate);
            }
            4 => self
                .makeup
                .set_target(db_to_lin(PARAMS[4].range.to_real(normalized))),
            _ => {}
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        let slope = 1.0 / self.ratio - 1.0; // dB of gain per dB over threshold
        for x in block.iter_mut() {
            let a = x.abs();
            let coeff = if a > self.env {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.env += coeff * (a - self.env);

            let over = lin_to_db(self.env) - self.threshold_db;
            let gr_db = if over > 0.0 { over * slope } else { 0.0 };
            *x *= db_to_lin(gr_db) * self.makeup.tick();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared() -> Compressor {
        let mut c = Compressor::new();
        c.prepare(SR);
        c
    }

    /// Steady-state peak of the processed tail of a sine at `amp`.
    fn settled_peak(c: &mut Compressor, amp: f32) -> f32 {
        let mut x: Vec<f32> = sine(SR, 220.0, SR as usize)
            .iter()
            .map(|s| s * amp)
            .collect();
        c.process(&mut x);
        assert_finite("comp output", &x);
        x[SR as usize / 2..]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn below_threshold_is_unity() {
        let mut c = prepared();
        // -30 dBFS sine under a -24 dB threshold.
        let peak = settled_peak(&mut c, db_to_lin(-30.0));
        let err_db = lin_to_db(peak) - -30.0;
        assert!(err_db.abs() < 0.2, "unity below threshold, off {err_db} dB");
    }

    #[test]
    fn static_curve_matches_ratio() {
        let mut c = prepared();
        // -6 dBFS into threshold -24, ratio 4: 18 dB over → 13.5 dB reduction.
        let peak = settled_peak(&mut c, db_to_lin(-6.0));
        let out_db = lin_to_db(peak);
        let expected = -6.0 - 18.0 * (1.0 - 1.0 / 4.0);
        assert!(
            (out_db - expected).abs() < 1.0,
            "expected ≈ {expected} dBFS, got {out_db}"
        );
    }

    #[test]
    fn higher_ratio_compresses_more() {
        let mut soft = prepared();
        soft.set_param(1, PARAMS[1].range.to_norm(2.0));
        let mut hard = prepared();
        hard.set_param(1, PARAMS[1].range.to_norm(12.0));
        let loud = db_to_lin(-6.0);
        assert!(
            settled_peak(&mut hard, loud) < settled_peak(&mut soft, loud) * 0.8,
            "ratio 12 must reduce more than ratio 2"
        );
    }

    #[test]
    fn makeup_gain_applies() {
        let mut c = prepared();
        c.set_param(4, PARAMS[4].range.to_norm(12.0));
        let peak = settled_peak(&mut c, db_to_lin(-30.0));
        let err_db = lin_to_db(peak) - (-30.0 + 12.0);
        assert!(err_db.abs() < 0.5, "makeup +12 dB, off {err_db} dB");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut c = prepared();
        let mut x = silence(4_096);
        c.process(&mut x);
        assert!(rms(&x) == 0.0);
    }
}
