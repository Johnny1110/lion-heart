//! Shared DSP building blocks — not effects themselves: signal primitives
//! (biquads, the 4× oversampler), the parameter smoother every effect leans
//! on (RT rule 6), and the lock-free asset hot-swap seam.

pub mod biquad;
pub mod grain;
pub mod oversample;
pub mod smooth;
pub mod swap;
pub mod wdf;

/// One-pole smoothing coefficient from a time constant in milliseconds
/// (~63% of the way per constant). 0 ms (or a zero rate) snaps.
#[inline]
pub fn onepole_ms(ms: f32, sample_rate: u32) -> f32 {
    if ms <= 0.0 || sample_rate == 0 {
        1.0
    } else {
        1.0 - (-1.0 / (ms * 1e-3 * sample_rate as f32)).exp()
    }
}

/// One-pole lowpass coefficient from a corner frequency in Hz.
#[inline]
pub fn onepole_hz(hz: f32, sample_rate: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * hz / sample_rate).exp()
}
