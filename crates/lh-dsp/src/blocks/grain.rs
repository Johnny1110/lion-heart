//! Granular (time-domain) pitch shifter — the classic "doppler" shifter:
//! an interpolated delay line read by two taps half a grain apart, each
//! windowed by a sine so the wrap of one tap hides under the other tap's
//! window peak. No FFT, RT-safe, and it works on any input (polyphonic
//! chords included) at the cost of a characteristic warble.
//!
//! The same math drives the shimmer reverb's octave regeneration
//! (`time::reverb`); this is the standalone, reusable form that the pitch
//! family builds on (ADR 016). `ratio` is the pitch multiplier: `2.0` is up
//! an octave, `0.5` is down an octave, `1.5` a fifth. `ratio == 1.0` is *not*
//! a bit-exact passthrough (the grain still crossfades), so callers that want
//! a clean signal keep a separate dry path.

/// Grain length. Long enough to track low guitar notes, short enough that the
/// crossfade stays a texture rather than an echo.
const GRAIN_MS: f32 = 64.0;

/// Fixed-capacity interpolated ring buffer: a fractional-sample read tap
/// behind the write head.
struct Line {
    buf: Vec<f32>,
    write: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            write: 0,
        }
    }

    /// (Re)allocate to `len` samples. Off the audio thread.
    fn resize(&mut self, len: usize) {
        self.buf = vec![0.0; len.max(4)];
        self.write = 0;
    }

    /// Interpolated read `delay_smp` behind the write head, clamped so the
    /// read can never wrap past it.
    #[inline]
    fn read_at(&self, delay_smp: f32) -> f32 {
        let len = self.buf.len();
        let d = delay_smp.clamp(1.0, (len - 2) as f32);
        let rp = self.write as f32 - d + len as f32;
        let i0 = rp as usize;
        let frac = rp - i0 as f32;
        let s0 = self.buf[i0 % len];
        let s1 = self.buf[(i0 + 1) % len];
        s0 + frac * (s1 - s0)
    }

    #[inline]
    fn push(&mut self, value: f32) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.buf.len();
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }
}

/// One voice of granular pitch shifting. Preallocate with [`GrainShift::prepare`]
/// before processing.
pub struct GrainShift {
    line: Line,
    /// Grain phasor in `[0, 1)`; its per-sample increment is set by `ratio`.
    phase: f32,
    grain_smp: f32,
}

impl GrainShift {
    pub fn new() -> Self {
        Self {
            line: Line::new(),
            phase: 0.0,
            grain_smp: 1.0,
        }
    }

    /// Size the grain buffer for a sample rate. Off the audio thread.
    pub fn prepare(&mut self, sample_rate: f32) {
        self.grain_smp = (GRAIN_MS * 1e-3 * sample_rate).max(1.0);
        // The farthest read is one grain plus a sample behind the write head.
        let cap = self.grain_smp as usize + 4;
        self.line.resize(cap);
        self.phase = 0.0;
    }

    /// One sample: push `x`, advance the grain, and return the shifted read.
    /// The read distance shrinks (up-shift, `ratio > 1`) or grows (down-shift,
    /// `ratio < 1`) across the grain; the two taps a half-grain apart keep the
    /// output continuous through each wrap.
    #[inline]
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        self.line.push(x);
        self.phase += (1.0 - ratio) / self.grain_smp;
        self.phase -= self.phase.floor(); // wrap into [0, 1)
        let p2 = {
            let p = self.phase + 0.5;
            p - p.floor()
        };
        let w1 = (std::f32::consts::PI * self.phase).sin();
        let w2 = (std::f32::consts::PI * p2).sin();
        let t1 = self.line.read_at(1.0 + self.phase * self.grain_smp);
        let t2 = self.line.read_at(1.0 + p2 * self.grain_smp);
        w1 * t1 + w2 * t2
    }

    /// Offset the grain phase into `[0, 1)` — decorrelates parallel shifters
    /// so their window seams don't line up.
    pub fn set_phase(&mut self, phase: f32) {
        self.phase = phase - phase.floor();
    }

    pub fn clear(&mut self) {
        self.line.clear();
        self.phase = 0.0;
    }
}

impl Default for GrainShift {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Projection magnitude onto `freq` over the settled tail (a Goertzel bin).
    fn tone_at(y: &[f32], freq: f32) -> f64 {
        let tail = &y[y.len() / 2..];
        let n = tail.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in tail.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(freq) * i as f64 / f64::from(SR);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        ((cs * 2.0 / n).powi(2) + (cc * 2.0 / n).powi(2)).sqrt()
    }

    fn shifted(ratio: f32, in_hz: f32) -> Vec<f32> {
        let mut g = GrainShift::new();
        g.prepare(SR);
        let len = SR as usize; // 1 s — well past the grain fill
        (0..len)
            .map(|i| {
                let x = (std::f32::consts::TAU * in_hz * i as f32 / SR).sin();
                g.process(x, ratio)
            })
            .collect()
    }

    #[test]
    fn up_ratio_lands_an_octave_above() {
        let y = shifted(2.0, 220.0);
        let up = tone_at(&y, 440.0);
        let orig = tone_at(&y, 220.0);
        assert!(up > 0.2, "up-octave present: {up:.3}");
        assert!(
            up > orig,
            "shifted tone dominates the original: {up:.3} vs {orig:.3}"
        );
    }

    #[test]
    fn down_ratio_lands_an_octave_below() {
        let y = shifted(0.5, 220.0);
        let down = tone_at(&y, 110.0);
        let orig = tone_at(&y, 220.0);
        assert!(down > 0.2, "sub-octave present: {down:.3}");
        assert!(
            down > orig,
            "shifted tone dominates the original: {down:.3} vs {orig:.3}"
        );
    }

    #[test]
    fn output_stays_finite_and_bounded() {
        let y = shifted(2.0, 220.0);
        assert!(y.iter().all(|s| s.is_finite()), "no NaN/inf");
        assert!(y.iter().all(|s| s.abs() < 4.0), "bounded near unity");
    }

    #[test]
    fn silence_in_silence_out() {
        let mut g = GrainShift::new();
        g.prepare(SR);
        let y: Vec<f32> = (0..2048).map(|_| g.process(0.0, 0.5)).collect();
        assert!(y.iter().all(|s| *s == 0.0), "silence stays silent");
    }
}
