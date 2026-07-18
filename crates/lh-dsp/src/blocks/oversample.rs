//! 4× oversampling for nonlinear stages (two cascaded 2× half-band stages).
//!
//! Waveshaping creates harmonics; anything past Nyquist folds back as
//! inharmonic aliasing. Running the shaper at 4× the rate and filtering on the
//! way down pushes the foldover ~2 octaves up and suppresses it by the filter
//! stopband (~70 dB for the 33-tap Blackman-windowed half-band used here).
//!
//! Total round-trip group delay is exactly 24 samples at the base rate
//! (16-tap center delay per stage: 8 + 4 + 4 + 8), asserted by tests.

const TAPS: usize = 33;
const HALF: usize = TAPS / 2; // 16, the filter's center index
/// Up/down round-trip latency at the base sample rate.
pub const LATENCY_SAMPLES: usize = 2 * (HALF / 2 + HALF / 4); // 24
/// Internal processing granularity; `process` chunks longer blocks.
pub const CHUNK: usize = 256;

/// Half-band lowpass (cutoff at half Nyquist), Blackman-windowed sinc.
/// Odd-distance taps carry the filter; even distances are zero by design.
fn design_halfband() -> Vec<f32> {
    use std::f32::consts::PI;
    (0..TAPS)
        .map(|n| {
            let k = n as isize - HALF as isize;
            let sinc = if k == 0 {
                0.5
            } else {
                (PI * k as f32 / 2.0).sin() / (PI * k as f32)
            };
            let t = n as f32 / (TAPS - 1) as f32;
            let window = 0.42 - 0.5 * (2.0 * PI * t).cos() + 0.08 * (4.0 * PI * t).cos();
            sinc * window
        })
        .collect()
}

/// 1 → 2 samples. Polyphase: the zero-stuffed convolution splits into an
/// even phase (h[0], h[2], …) and an odd phase (h[1], h[3], …), each running
/// at the input rate. Taps are scaled ×2 to preserve amplitude.
struct Upsampler2x {
    even: Vec<f32>, // 17 taps
    odd: Vec<f32>,  // 16 taps
    hist: Vec<f32>, // last HALF input samples
    ext: Vec<f32>,  // hist ++ current chunk, rebuilt per call
}

impl Upsampler2x {
    /// `max_input`: largest chunk this stage will ever see — the scratch is
    /// sized for it up front so `process` never allocates (RT rule 1).
    fn new(max_input: usize) -> Self {
        let h = design_halfband();
        Self {
            even: h.iter().step_by(2).map(|c| c * 2.0).collect(),
            odd: h.iter().skip(1).step_by(2).map(|c| c * 2.0).collect(),
            hist: vec![0.0; HALF],
            ext: Vec::with_capacity(HALF + max_input),
        }
    }

    fn reset(&mut self) {
        self.hist.iter_mut().for_each(|s| *s = 0.0);
    }

    /// `out` must be exactly `2 * input.len()`.
    fn process(&mut self, input: &[f32], out: &mut [f32]) {
        debug_assert_eq!(out.len(), input.len() * 2);
        debug_assert!(
            self.ext.capacity() >= HALF + input.len(),
            "scratch undersized"
        );
        self.ext.clear();
        self.ext.extend_from_slice(&self.hist);
        self.ext.extend_from_slice(input);
        for i in 0..input.len() {
            // Window covering x[i-16] ..= x[i]; w[HALF - j] == x[i - j].
            // Each polyphase branch is a plain FIR over x: y[2i] = Σ even[j]·x[i-j],
            // y[2i+1] = Σ odd[j]·x[i-j].
            let w = &self.ext[i..i + HALF + 1];
            let mut even_acc = 0.0f32;
            for (j, c) in self.even.iter().enumerate() {
                even_acc += c * w[HALF - j];
            }
            let mut odd_acc = 0.0f32;
            for (j, c) in self.odd.iter().enumerate() {
                odd_acc += c * w[HALF - j];
            }
            out[2 * i] = even_acc;
            out[2 * i + 1] = odd_acc;
        }
        let n = self.ext.len();
        self.hist.copy_from_slice(&self.ext[n - HALF..]);
    }
}

/// 2 → 1 samples: full convolution evaluated at even output positions.
struct Downsampler2x {
    taps: Vec<f32>,
    hist: Vec<f32>, // last TAPS-1 input samples (at the higher rate)
    ext: Vec<f32>,
}

impl Downsampler2x {
    /// `max_input`: largest (higher-rate) chunk this stage will ever see.
    fn new(max_input: usize) -> Self {
        Self {
            taps: design_halfband(),
            hist: vec![0.0; TAPS - 1],
            ext: Vec::with_capacity(TAPS - 1 + max_input),
        }
    }

    fn reset(&mut self) {
        self.hist.iter_mut().for_each(|s| *s = 0.0);
    }

    /// `input.len()` must be exactly `2 * out.len()`.
    fn process(&mut self, input: &[f32], out: &mut [f32]) {
        debug_assert_eq!(input.len(), out.len() * 2);
        debug_assert!(
            self.ext.capacity() >= TAPS - 1 + input.len(),
            "scratch undersized"
        );
        self.ext.clear();
        self.ext.extend_from_slice(&self.hist);
        self.ext.extend_from_slice(input);
        for (i, o) in out.iter_mut().enumerate() {
            // Window covering v[2i-32] ..= v[2i]; w[TAPS-1-j] == v[2i - j].
            let w = &self.ext[2 * i..2 * i + TAPS];
            let mut acc = 0.0f32;
            for (j, c) in self.taps.iter().enumerate() {
                acc += c * w[TAPS - 1 - j];
            }
            *o = acc;
        }
        let n = self.ext.len();
        self.hist.copy_from_slice(&self.ext[n - (TAPS - 1)..]);
    }
}

/// Run a memoryless (or stateful) shaper at 4× the base sample rate.
pub struct Oversampler4x {
    up1: Upsampler2x,
    up2: Upsampler2x,
    down2: Downsampler2x,
    down1: Downsampler2x,
    buf2: Vec<f32>,
    buf4: Vec<f32>,
}

impl Default for Oversampler4x {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler4x {
    pub fn new() -> Self {
        // Each stage sees a different chunk length: up1 CHUNK → up2 2×CHUNK
        // → shaper 4×CHUNK → down2 4×CHUNK → down1 2×CHUNK.
        Self {
            up1: Upsampler2x::new(CHUNK),
            up2: Upsampler2x::new(2 * CHUNK),
            down2: Downsampler2x::new(4 * CHUNK),
            down1: Downsampler2x::new(2 * CHUNK),
            buf2: vec![0.0; 2 * CHUNK],
            buf4: vec![0.0; 4 * CHUNK],
        }
    }

    pub fn reset(&mut self) {
        self.up1.reset();
        self.up2.reset();
        self.down2.reset();
        self.down1.reset();
    }

    /// Process `block` in place; `shape` sees the signal at 4× rate.
    pub fn process(&mut self, block: &mut [f32], mut shape: impl FnMut(&mut [f32])) {
        for chunk in block.chunks_mut(CHUNK) {
            let n = chunk.len();
            self.up1.process(chunk, &mut self.buf2[..2 * n]);
            self.up2
                .process(&self.buf2[..2 * n], &mut self.buf4[..4 * n]);
            shape(&mut self.buf4[..4 * n]);
            self.down2
                .process(&self.buf4[..4 * n], &mut self.buf2[..2 * n]);
            self.down1.process(&self.buf2[..2 * n], chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, sine};

    #[test]
    fn identity_impulse_lands_at_documented_latency() {
        let mut os = Oversampler4x::new();
        let mut x = vec![0.0f32; 256];
        x[10] = 1.0;
        os.process(&mut x, |_| {});
        assert_finite("impulse response", &x);

        let (argmax, peak) = x.iter().enumerate().fold((0, 0.0f32), |(bi, bv), (i, v)| {
            if v.abs() > bv { (i, v.abs()) } else { (bi, bv) }
        });
        assert_eq!(argmax, 10 + LATENCY_SAMPLES, "group delay must be exact");
        // An impulse has a flat spectrum; the filters' transition band near
        // Nyquist shaves some of it, so the peak lands below 1.0. Unity gain
        // in the audible band is asserted by the sine round-trip test.
        assert!((0.85..=1.02).contains(&peak), "peak out of range: {peak}");
    }

    #[test]
    fn identity_sine_survives_round_trip() {
        let sr = 48_000;
        let x = sine(sr, 1_000.0, 4_096);
        let mut y = x.clone();
        let mut os = Oversampler4x::new();
        os.process(&mut y, |_| {});

        // Compare y[n] against x[n - LATENCY] on the interior.
        let mut err = 0.0f64;
        let mut sig = 0.0f64;
        for n in 512..4_096 {
            let want = x[n - LATENCY_SAMPLES];
            let got = y[n];
            err += f64::from(got - want) * f64::from(got - want);
            sig += f64::from(want) * f64::from(want);
        }
        let snr_db = 10.0 * (sig / err.max(1e-30)).log10();
        assert!(snr_db > 50.0, "passband SNR too low: {snr_db:.1} dB");
    }

    #[test]
    fn shape_runs_at_four_times_rate() {
        let mut os = Oversampler4x::new();
        let mut counted = 0usize;
        let mut x = vec![0.0f32; 300]; // spans two internal chunks
        os.process(&mut x, |os_buf| counted += os_buf.len());
        assert_eq!(counted, 1_200);
    }
}
