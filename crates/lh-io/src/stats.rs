//! Lock-free counters shared between the audio callbacks and reporting threads.

use std::sync::atomic::{AtomicU64, Ordering};

/// Counters written from the real-time audio callbacks. Plain atomics only —
/// the audio thread never locks; readers take a [`Snapshot`] at their own pace.
#[derive(Debug, Default)]
pub struct Stats {
    pub in_callbacks: AtomicU64,
    pub out_callbacks: AtomicU64,
    pub in_frames: AtomicU64,
    pub out_frames: AtomicU64,
    /// Frames dropped because the ring buffer was full (input side).
    pub overrun_frames: AtomicU64,
    /// Input callbacks that dropped at least one frame.
    pub overrun_events: AtomicU64,
    /// Frames substituted with silence because the ring buffer was empty.
    pub underrun_frames: AtomicU64,
    /// Output callbacks that were short at least one frame.
    pub underrun_events: AtomicU64,
    /// Stream error callbacks (device disconnected, backend failure, …).
    pub stream_errors: AtomicU64,
    /// Worst observed callback duration in nanoseconds (input and output combined).
    pub max_callback_nanos: AtomicU64,
}

impl Stats {
    pub fn record_callback_nanos(&self, nanos: u64) {
        self.max_callback_nanos.fetch_max(nanos, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            in_callbacks: self.in_callbacks.load(Ordering::Relaxed),
            out_callbacks: self.out_callbacks.load(Ordering::Relaxed),
            in_frames: self.in_frames.load(Ordering::Relaxed),
            out_frames: self.out_frames.load(Ordering::Relaxed),
            overrun_frames: self.overrun_frames.load(Ordering::Relaxed),
            overrun_events: self.overrun_events.load(Ordering::Relaxed),
            underrun_frames: self.underrun_frames.load(Ordering::Relaxed),
            underrun_events: self.underrun_events.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
            max_callback_nanos: self.max_callback_nanos.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time copy of [`Stats`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub in_callbacks: u64,
    pub out_callbacks: u64,
    pub in_frames: u64,
    pub out_frames: u64,
    pub overrun_frames: u64,
    pub overrun_events: u64,
    pub underrun_frames: u64,
    pub underrun_events: u64,
    pub stream_errors: u64,
    pub max_callback_nanos: u64,
}

impl Snapshot {
    /// True when any audible-glitch indicator moved.
    pub fn has_xruns(&self) -> bool {
        self.underrun_events > 0 || self.overrun_events > 0 || self.stream_errors > 0
    }

    pub fn max_callback_millis(&self) -> f64 {
        self.max_callback_nanos as f64 / 1e6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_counters_and_xruns() {
        let stats = Stats::default();
        assert!(!stats.snapshot().has_xruns());

        stats.in_frames.fetch_add(64, Ordering::Relaxed);
        stats.underrun_events.fetch_add(1, Ordering::Relaxed);
        stats.record_callback_nanos(1_500_000);
        stats.record_callback_nanos(400_000); // must not lower the max

        let snap = stats.snapshot();
        assert_eq!(snap.in_frames, 64);
        assert!(snap.has_xruns());
        assert_eq!(snap.max_callback_nanos, 1_500_000);
        assert!((snap.max_callback_millis() - 1.5).abs() < 1e-9);
    }
}
