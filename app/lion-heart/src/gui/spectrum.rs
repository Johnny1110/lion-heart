//! Output spectrum analyzer (PRD 003): fed from the engine's post-output
//! tap, FFT'd **on the GUI thread** (never the audio path), displayed as
//! log-frequency bins with fast-attack / slow-release ballistics.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// Analysis window (~85 ms at 48 kHz — enough low-end resolution to place
/// a 30 Hz rumble while still tracking playing).
const FFT_LEN: usize = 4_096;
/// Log-spaced display bins across 20 Hz – 20 kHz.
pub const DISPLAY_BINS: usize = 120;
pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
/// Display floor; bins rest here when silent.
pub const DB_FLOOR: f32 = -90.0;
/// Release per update call (~30 Hz updates ⇒ ~18 dB/s decay).
const RELEASE_DB: f32 = 0.6;

pub struct SpectrumAnalyzer {
    sample_rate: f32,
    /// Ring of the latest FFT_LEN samples from the tap.
    window: Vec<f32>,
    write: usize,
    hann: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// FFT-bin range per display bin.
    ranges: Vec<(usize, usize)>,
    /// Display values in dBFS with ballistics applied.
    pub bins: Vec<f32>,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate as f32;
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_LEN);
        let hann: Vec<f32> = (0..FFT_LEN)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / FFT_LEN as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();
        // Each display bin covers the FFT bins inside its log-frequency
        // span (at least one; low bins overlap-share their nearest).
        let bin_hz = sample_rate / FFT_LEN as f32;
        let edge = |i: usize| -> f32 {
            FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf(i as f32 / DISPLAY_BINS as f32)
        };
        let ranges = (0..DISPLAY_BINS)
            .map(|i| {
                let lo = (edge(i) / bin_hz).floor().max(1.0) as usize;
                let hi = ((edge(i + 1) / bin_hz).ceil() as usize).clamp(lo + 1, FFT_LEN / 2 + 1);
                (lo.min(FFT_LEN / 2), hi)
            })
            .collect();
        Self {
            sample_rate,
            window: vec![0.0; FFT_LEN],
            write: 0,
            hann,
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            ranges,
            bins: vec![DB_FLOOR; DISPLAY_BINS],
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Append tapped samples into the sliding window.
    pub fn feed(&mut self, samples: &[f32]) {
        for &s in samples {
            self.window[self.write] = s;
            self.write = (self.write + 1) % self.window.len();
        }
    }

    /// Recompute the display bins from the latest window (call ~30 Hz).
    pub fn update(&mut self) {
        // Unroll the ring in time order, windowed.
        let n = self.window.len();
        for (i, x) in self.input.iter_mut().enumerate() {
            *x = self.window[(self.write + i) % n] * self.hann[i];
        }
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return;
        }
        // Hann coherent gain is 0.5: a full-scale sine peaks at 0 dBFS.
        let scale = 4.0 / FFT_LEN as f32;
        for (bin, &(lo, hi)) in self.bins.iter_mut().zip(&self.ranges) {
            let mut peak = 0.0f32;
            for c in &self.output[lo..hi] {
                peak = peak.max(c.norm_sqr());
            }
            let amp = peak.sqrt() * scale;
            let db = (20.0 * amp.max(1e-9).log10()).max(DB_FLOOR);
            // Fast attack, slow release.
            *bin = if db > *bin {
                db
            } else {
                (*bin - RELEASE_DB).max(db).max(DB_FLOOR)
            };
        }
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
        let target = (1_000.0f32 / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln();
        let bin = (target * DISPLAY_BINS as f32) as usize;
        let peak = analyzer.bins[bin.saturating_sub(1)..(bin + 2).min(DISPLAY_BINS)]
            .iter()
            .fold(f32::MIN, |m, v| m.max(*v));
        assert!(
            peak > -3.0 && peak < 1.0,
            "1 kHz full-scale sine should read ≈0 dBFS, got {peak}"
        );
        // Far-away bins stay near the floor.
        assert!(
            analyzer.bins[10] < -40.0,
            "low bins quiet: {}",
            analyzer.bins[10]
        );
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
        assert!(
            analyzer.bins.iter().all(|&b| b <= DB_FLOOR + 1e-3),
            "all bins must decay to the floor"
        );
    }
}
