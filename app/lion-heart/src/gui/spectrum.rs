//! Output spectrum analyzer (PRD 003): fed from the engine's post-output
//! tap, FFT'd **on the GUI thread** (never the audio path), displayed as
//! log-frequency bins with fast-attack / slow-release ballistics.

use super::gpu::fft::{self, GpuFft};

/// Analysis window (~85 ms at 48 kHz — enough low-end resolution to place
/// a 30 Hz rumble while still tracking playing).
#[allow(dead_code)]
pub const FFT_LEN: usize = fft::FFT_LEN;
/// Log-spaced display bins across 20 Hz – 20 kHz.
#[allow(dead_code)]
pub const DISPLAY_BINS: usize = fft::DISPLAY_BINS;
#[allow(dead_code)]
pub const FREQ_MIN: f32 = fft::FREQ_MIN;
#[allow(dead_code)]
pub const FREQ_MAX: f32 = fft::FREQ_MAX;
/// Display floor; bins rest here when silent.
pub const DB_FLOOR: f32 = fft::DB_FLOOR;

pub struct SpectrumAnalyzer {
    inner: GpuFft,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            inner: GpuFft::new(sample_rate),
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.inner.sample_rate()
    }

    /// Append tapped samples into the sliding window.
    pub fn feed(&mut self, samples: &[f32]) {
        self.inner.feed(samples)
    }

    /// Recompute the display bins from the latest window (call ~30 Hz).
    pub fn update(&mut self) {
        self.inner.update()
    }

    /// Direct access for views and assertions.
    pub fn bins(&self) -> &[f32] {
        self.inner.bins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_sine_reads_near_zero_dbfs_at_its_bin() {
        let mut analyzer = SpectrumAnalyzer::new(48_000);
        let sine: Vec<f32> = (0..FFT_LEN * 2)
            .map(|i| (std::f32::consts::TAU * 1_000.0 * i as f32 / 48_000.0).sin())
            .collect();
        analyzer.feed(&sine);
        analyzer.update();
        let bins = analyzer.bins();
        let target = (1_000.0f32 / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln();
        let bin = (target * DISPLAY_BINS as f32) as usize;
        let peak = bins[bin.saturating_sub(1)..(bin + 2).min(DISPLAY_BINS)]
            .iter()
            .fold(f32::MIN, |m, v| m.max(*v));
        assert!(
            peak > -3.0 && peak < 1.0,
            "1 kHz full-scale sine should read ≈0 dBFS, got {peak}"
        );
        // Far-away bins stay near the floor.
        assert!(bins[10] < -40.0, "low bins quiet: {}", bins[10]);
    }

    #[test]
    fn silence_decays_to_the_floor() {
        let mut analyzer = SpectrumAnalyzer::new(48_000);
        let sine: Vec<f32> = (0..FFT_LEN)
            .map(|i| (std::f32::consts::TAU * 500.0 * i as f32 / 48_000.0).sin())
            .collect();
        analyzer.feed(&sine);
        analyzer.update();
        analyzer.feed(&vec![0.0; FFT_LEN]);
        for _ in 0..400 {
            analyzer.update();
        }
        let bins = analyzer.bins();
        assert!(
            bins.iter().all(|&b| b <= DB_FLOOR + 1e-3),
            "all bins must decay to the floor"
        );
    }
}
