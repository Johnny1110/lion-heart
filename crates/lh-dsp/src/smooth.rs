//! One-pole parameter smoothing — the layer that makes live knob turns
//! click-free (white paper §4.3: nothing reaching the signal path may jump).

/// Exponential approach toward a target value; ~63% of the way per time
/// constant, effectively settled after ~5 time constants.
#[derive(Debug, Clone, Copy)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    pub fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            coeff: 1.0,
        }
    }

    /// Set the time constant. 0 ms = snap instantly.
    pub fn configure(&mut self, ms: f32, sample_rate: u32) {
        self.coeff = if ms <= 0.0 || sample_rate == 0 {
            1.0
        } else {
            1.0 - (-1.0 / (ms * 1e-3 * sample_rate as f32)).exp()
        };
    }

    pub fn set_target(&mut self, value: f32) {
        self.target = value;
        if self.coeff >= 1.0 {
            self.current = value;
        }
    }

    pub fn snap_to_target(&mut self) {
        self.current = self.target;
    }

    /// Advance one sample and return the smoothed value.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        let next = self.current + self.coeff * (self.target - self.current);
        // Snap once the residual is inaudible (-80 dB relative), and also on
        // the f32 stall (increment below the ULP of `current`, so `next`
        // stops moving). Guarantees `is_settled` terminates and keeps the
        // approach to zero out of subnormal territory.
        let eps = 1e-4 * self.target.abs().max(1e-5);
        if next == self.current || (self.target - next).abs() <= eps {
            self.current = self.target;
        } else {
            self.current = next;
        }
        self.current
    }

    pub fn current(&self) -> f32 {
        self.current
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_settled(&self) -> bool {
        self.current == self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_when_unconfigured_or_zero_ms() {
        let mut s = Smoothed::new(0.0);
        s.set_target(1.0);
        assert_eq!(s.tick(), 1.0);

        let mut s = Smoothed::new(0.0);
        s.configure(0.0, 48_000);
        s.set_target(0.5);
        assert_eq!(s.tick(), 0.5);
    }

    #[test]
    fn converges_monotonically_within_five_time_constants() {
        let sr = 48_000;
        let mut s = Smoothed::new(0.0);
        s.configure(10.0, sr); // 10 ms => 480 samples per time constant
        s.set_target(1.0);

        let mut prev = 0.0;
        for _ in 0..480 * 5 {
            let v = s.tick();
            assert!(v >= prev, "no overshoot, monotone approach");
            prev = v;
        }
        assert!(prev > 0.99, "settled after 5 time constants, got {prev}");
        for _ in 0..480 * 5 {
            s.tick();
        }
        assert!(s.is_settled());
    }

    #[test]
    fn one_time_constant_reaches_about_63_percent() {
        let sr = 48_000;
        let mut s = Smoothed::new(0.0);
        s.configure(10.0, sr);
        s.set_target(1.0);
        let mut v = 0.0;
        for _ in 0..480 {
            v = s.tick();
        }
        assert!((v - 0.632).abs() < 0.01, "got {v}");
    }
}
