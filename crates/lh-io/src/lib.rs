//! Audio I/O foundation for Lion-Heart (milestone M0).
//!
//! Everything cpal-specific lives inside this crate (white paper §5.2): device
//! enumeration/selection, duplex passthrough streaming, xrun accounting, and
//! round-trip latency measurement. Other crates must not name cpal types.

pub mod devices;
pub mod latency;
pub mod passthrough;
pub mod stats;

mod stream;

use thiserror::Error;

/// Default engine sample rate. NAM models are rate-locked to their training
/// rate, which is almost always 48 kHz (white paper §5.3).
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Error)]
pub enum IoError {
    #[error("device not found: {0:?} (run `lion-heart devices` to list devices)")]
    DeviceNotFound(String),

    #[error("device {0:?} does not support {1}")]
    DirectionUnsupported(String, &'static str),

    #[error("no default {0} device")]
    NoDefaultDevice(&'static str),

    #[error("input channel {requested} is out of range: device has {available} channel(s)")]
    BadChannel { requested: u16, available: u16 },

    #[error("device sample format is {0}, but only f32 is supported in M0")]
    UnsupportedFormat(String),

    #[error(
        "no loopback signal detected (noise floor {noise:.4}, threshold {threshold:.4}).\n\
         Connect the interface output back into the measured input with a cable\n\
         (or enable the interface's loopback mode) and check input gain."
    )]
    NoLoopbackSignal { noise: f32, threshold: f32 },

    #[error(transparent)]
    Backend(#[from] cpal::Error),
}
