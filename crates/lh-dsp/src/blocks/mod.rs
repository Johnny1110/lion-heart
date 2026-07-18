//! Shared DSP building blocks — not effects themselves: signal primitives
//! (biquads, the 4× oversampler), the parameter smoother every effect leans
//! on (RT rule 6), and the lock-free asset hot-swap seam.

pub mod biquad;
pub mod oversample;
pub mod smooth;
pub mod swap;
