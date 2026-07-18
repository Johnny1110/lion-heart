//! Second-order IIR sections (RBJ cookbook), Direct Form II transposed.
//! Shared by the EQ and any future filter-based effect. Coefficients are
//! recomputed at block rate by the owning effect; processing is per-sample.

/// One biquad section. Unity by default (passes audio through).
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Peaking EQ centered on `fc` with `gain_db` boost/cut.
    pub fn set_peaking(&mut self, sample_rate: f32, fc: f32, gain_db: f32, q: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let alpha = w.sin() / (2.0 * q);
        let cos_w = w.cos();
        self.set(
            1.0 + alpha * a,
            -2.0 * cos_w,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos_w,
            1.0 - alpha / a,
        );
    }

    /// Low shelf with corner `fc`, slope 1.
    pub fn set_low_shelf(&mut self, sample_rate: f32, fc: f32, gain_db: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let (sin_w, cos_w) = w.sin_cos();
        let alpha = sin_w / 2.0 * std::f32::consts::SQRT_2;
        let sq = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) - (a - 1.0) * cos_w + sq),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w),
            a * ((a + 1.0) - (a - 1.0) * cos_w - sq),
            (a + 1.0) + (a - 1.0) * cos_w + sq,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos_w),
            (a + 1.0) + (a - 1.0) * cos_w - sq,
        );
    }

    /// Highpass (12 dB/oct); `q` shapes the corner (0.707 = Butterworth).
    pub fn set_highpass(&mut self, sample_rate: f32, fc: f32, q: f32) {
        let w = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let (sin_w, cos_w) = w.sin_cos();
        let alpha = sin_w / (2.0 * q);
        let b = (1.0 + cos_w) / 2.0;
        self.set(b, -(1.0 + cos_w), b, 1.0 + alpha, -2.0 * cos_w, 1.0 - alpha);
    }

    /// Lowpass (12 dB/oct); `q` shapes the corner (0.707 = Butterworth).
    pub fn set_lowpass(&mut self, sample_rate: f32, fc: f32, q: f32) {
        let w = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let (sin_w, cos_w) = w.sin_cos();
        let alpha = sin_w / (2.0 * q);
        let b = (1.0 - cos_w) / 2.0;
        self.set(b, 1.0 - cos_w, b, 1.0 + alpha, -2.0 * cos_w, 1.0 - alpha);
    }

    /// Magnitude response at `freq` in dB, straight from the coefficients —
    /// what the GUI curve plots is exactly what the filter does.
    pub fn magnitude_db(&self, sample_rate: f32, freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let (sin_w, cos_w) = w.sin_cos();
        let (sin_2w, cos_2w) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * cos_w + self.b2 * cos_2w;
        let num_im = -(self.b1 * sin_w + self.b2 * sin_2w);
        let den_re = 1.0 + self.a1 * cos_w + self.a2 * cos_2w;
        let den_im = -(self.a1 * sin_w + self.a2 * sin_2w);
        let num = (num_re * num_re + num_im * num_im).max(1e-20);
        let den = (den_re * den_re + den_im * den_im).max(1e-20);
        10.0 * (num / den).log10()
    }

    /// High shelf with corner `fc`, slope 1.
    pub fn set_high_shelf(&mut self, sample_rate: f32, fc: f32, gain_db: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let (sin_w, cos_w) = w.sin_cos();
        let alpha = sin_w / 2.0 * std::f32::consts::SQRT_2;
        let sq = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) + (a - 1.0) * cos_w + sq),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w),
            a * ((a + 1.0) + (a - 1.0) * cos_w - sq),
            (a + 1.0) - (a - 1.0) * cos_w + sq,
            2.0 * ((a - 1.0) - (a + 1.0) * cos_w),
            (a + 1.0) - (a - 1.0) * cos_w - sq,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{rms, sine};
    use lh_core::lin_to_db;

    const SR: f32 = 48_000.0;

    /// Steady-state gain (dB) of `filter` at `freq`.
    fn gain_at(filter: &mut Biquad, freq: f32) -> f32 {
        let x = sine(SR as u32, freq, SR as usize / 2);
        let mut y = x.clone();
        for s in y.iter_mut() {
            *s = filter.process_sample(*s);
        }
        let n = y.len();
        lin_to_db(rms(&y[n / 2..]) / rms(&x[n / 2..]))
    }

    #[test]
    fn unity_by_default() {
        let mut f = Biquad::default();
        assert!(gain_at(&mut f, 1_000.0).abs() < 1e-3);
    }

    #[test]
    fn peaking_boosts_at_center_only() {
        let mut f = Biquad::default();
        f.set_peaking(SR, 800.0, 9.0, 0.8);
        assert!((gain_at(&mut f, 800.0) - 9.0).abs() < 0.3);
        f.reset();
        assert!(gain_at(&mut f, 60.0).abs() < 1.0);
        f.reset();
        assert!(gain_at(&mut f, 10_000.0).abs() < 1.0);
    }

    #[test]
    fn cut_filters_slope_off_their_side() {
        let mut hp = Biquad::default();
        hp.set_highpass(SR, 100.0, 0.707);
        assert!(gain_at(&mut hp, 25.0) < -20.0);
        hp.reset();
        assert!(gain_at(&mut hp, 1_000.0).abs() < 0.5);

        let mut lp = Biquad::default();
        lp.set_lowpass(SR, 5_000.0, 0.707);
        assert!(gain_at(&mut lp, 18_000.0) < -18.0);
        lp.reset();
        assert!(gain_at(&mut lp, 500.0).abs() < 0.5);
    }

    #[test]
    fn magnitude_matches_rendered_gain() {
        let mut f = Biquad::default();
        f.set_peaking(SR, 800.0, 9.0, 0.8);
        for freq in [100.0, 800.0, 3_000.0] {
            let analytic = f.magnitude_db(SR, freq);
            let rendered = gain_at(&mut f, freq);
            f.reset();
            assert!(
                (analytic - rendered).abs() < 0.3,
                "{freq} Hz: analytic {analytic} vs rendered {rendered}"
            );
        }
    }

    #[test]
    fn shelves_boost_their_side() {
        let mut low = Biquad::default();
        low.set_low_shelf(SR, 120.0, 12.0);
        assert!((gain_at(&mut low, 40.0) - 12.0).abs() < 1.0);
        low.reset();
        assert!(gain_at(&mut low, 5_000.0).abs() < 0.5);

        let mut high = Biquad::default();
        high.set_high_shelf(SR, 3_200.0, -9.0);
        assert!((gain_at(&mut high, 10_000.0) - -9.0).abs() < 1.0);
        high.reset();
        assert!(gain_at(&mut high, 100.0).abs() < 0.5);
    }
}
