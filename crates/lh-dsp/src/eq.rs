//! Post-amp EQ: low shelf (120 Hz) + sweepable mid peak + high shelf
//! (3.2 kHz). Gains and mid frequency are smoothed; biquad coefficients are
//! recomputed once per block from the smoothed values — at 48 kHz / 64
//! samples that is a 750 Hz update rate, far above audibility of zipper
//! noise, and it keeps transcendental math out of the per-sample loop.

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::biquad::Biquad;
use crate::smooth::Smoothed;

const LOW_SHELF_HZ: f32 = 120.0;
const HIGH_SHELF_HZ: f32 = 3_200.0;
const MID_Q: f32 = 0.8;

static PARAMS: [ParamDesc; 4] = [
    ParamDesc {
        key: "low",
        name: "Low",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "mid",
        name: "Mid",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "freq",
        name: "Mid Freq",
        unit: "Hz",
        range: Range::Log {
            min: 200.0,
            max: 5_000.0,
        },
        default: 800.0,
        smoothing_ms: 60.0,
    },
    ParamDesc {
        key: "high",
        name: "High",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 30.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "eq",
    name: "EQ",
    params: &PARAMS,
};

/// Single-pedal family: the pedal key doubles as the family key.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "eq",
    name: "EQ",
    pedals: &[&DESC],
};

pub struct Eq {
    sample_rate: f32,
    low_db: Smoothed,
    mid_db: Smoothed,
    mid_freq: Smoothed,
    high_db: Smoothed,
    low: [Biquad; 2],
    mid: [Biquad; 2],
    high: [Biquad; 2],
}

impl Default for Eq {
    fn default() -> Self {
        Self::new()
    }
}

impl Eq {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            low_db: Smoothed::new(PARAMS[0].default),
            mid_db: Smoothed::new(PARAMS[1].default),
            mid_freq: Smoothed::new(PARAMS[2].default),
            high_db: Smoothed::new(PARAMS[3].default),
            low: [Biquad::default(); 2],
            mid: [Biquad::default(); 2],
            high: [Biquad::default(); 2],
        }
    }

    /// Advance smoothed controls by `n` samples and rebuild coefficients.
    fn update_coeffs(&mut self, n: usize) {
        for _ in 0..n {
            self.low_db.tick();
            self.mid_db.tick();
            self.mid_freq.tick();
            self.high_db.tick();
        }
        for ch in 0..2 {
            self.low[ch].set_low_shelf(self.sample_rate, LOW_SHELF_HZ, self.low_db.current());
            self.mid[ch].set_peaking(
                self.sample_rate,
                self.mid_freq.current(),
                self.mid_db.current(),
                MID_Q,
            );
            self.high[ch].set_high_shelf(self.sample_rate, HIGH_SHELF_HZ, self.high_db.current());
        }
    }
}

impl Effect for Eq {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        for (smoothed, desc) in [
            (&mut self.low_db, &PARAMS[0]),
            (&mut self.mid_db, &PARAMS[1]),
            (&mut self.mid_freq, &PARAMS[2]),
            (&mut self.high_db, &PARAMS[3]),
        ] {
            smoothed.configure(desc.smoothing_ms, sample_rate);
            smoothed.snap_to_target();
        }
        self.update_coeffs(0);
        self.reset();
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            self.low[ch].reset();
            self.mid[ch].reset();
            self.high[ch].reset();
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let real = PARAMS[index].range.to_real(normalized);
        match index {
            0 => self.low_db.set_target(real),
            1 => self.mid_db.set_target(real),
            2 => self.mid_freq.set_target(real),
            3 => self.high_db.set_target(real),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.update_coeffs(left.len());
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut y = self.low[0].process_sample(*l);
            y = self.mid[0].process_sample(y);
            *l = self.high[0].process_sample(y);
            let mut y = self.low[1].process_sample(*r);
            y = self.mid[1].process_sample(y);
            *r = self.high[1].process_sample(y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, rms, silence, sine};
    use lh_core::lin_to_db;

    const SR: u32 = 48_000;

    fn prepared() -> Eq {
        let mut eq = Eq::new();
        eq.prepare(SR);
        eq
    }

    fn set(eq: &mut Eq, index: usize, real: f32) {
        eq.set_param(index, PARAMS[index].range.to_norm(real));
    }

    /// Steady-state gain (dB) at `freq` through the whole EQ.
    fn gain_at(eq: &mut Eq, freq: f32) -> f32 {
        let x = sine(SR, freq, SR as usize / 2);
        let mut y = x.clone();
        let mut yr = x.clone();
        for (l, r) in y.chunks_mut(64).zip(yr.chunks_mut(64)) {
            eq.process(l, r);
        }
        assert_finite("eq output", &y);
        let n = y.len();
        lin_to_db(rms(&y[n / 2..]) / rms(&x[n / 2..]))
    }

    #[test]
    fn flat_settings_pass_through() {
        let mut eq = prepared();
        for freq in [60.0, 250.0, 1_000.0, 4_000.0, 12_000.0] {
            let g = gain_at(&mut eq, freq);
            assert!(
                g.abs() < 0.05,
                "flat EQ must be unity at {freq} Hz, got {g}"
            );
            eq.reset();
        }
    }

    #[test]
    fn low_shelf_boosts_lows_only() {
        let mut eq = prepared();
        set(&mut eq, 0, 12.0);
        assert!((gain_at(&mut eq, 50.0) - 12.0).abs() < 1.5);
        eq.reset();
        assert!(gain_at(&mut eq, 4_000.0).abs() < 1.0);
    }

    #[test]
    fn mid_cut_lands_on_the_swept_center() {
        let mut eq = prepared();
        set(&mut eq, 1, -12.0);
        set(&mut eq, 2, 1_500.0);
        // Let the 60 ms frequency smoothing settle before measuring.
        let mut warm = sine(SR, 1_500.0, SR as usize / 2);
        let mut warm_r = warm.clone();
        for (l, r) in warm.chunks_mut(64).zip(warm_r.chunks_mut(64)) {
            eq.process(l, r);
        }
        assert!((gain_at(&mut eq, 1_500.0) - -12.0).abs() < 1.0);
        eq.reset();
        assert!(gain_at(&mut eq, 150.0).abs() > -2.0);
    }

    #[test]
    fn high_shelf_cuts_highs_only() {
        let mut eq = prepared();
        set(&mut eq, 3, -9.0);
        assert!((gain_at(&mut eq, 10_000.0) - -9.0).abs() < 1.5);
        eq.reset();
        assert!(gain_at(&mut eq, 120.0).abs() < 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut eq = prepared();
        set(&mut eq, 0, 12.0);
        set(&mut eq, 3, -12.0);
        let mut x = silence(4_096);
        let mut xr = silence(4_096);
        eq.process(&mut x, &mut xr);
        assert!(rms(&x) == 0.0 && rms(&xr) == 0.0);
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut eq = Eq::new();
            eq.prepare(sr);
            set(&mut eq, 0, 8.0);
            set(&mut eq, 1, -6.0);
            set(&mut eq, 3, 4.0);
            for chunk in [32usize, 483, 1_024] {
                let mut x = sine(sr, 440.0, 4_096);
                let mut xr = x.clone();
                for (l, r) in x.chunks_mut(chunk).zip(xr.chunks_mut(chunk)) {
                    eq.process(l, r);
                }
                assert_finite("eq multirate", &x);
            }
        }
    }
}
