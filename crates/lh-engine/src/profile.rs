//! Per-slot DSP timing for the chain (PRD 019-adjacent diagnostics).
//!
//! Answers the question the aggregate callback counters cannot: *which pedal
//! is eating the block?* `lh_io::stats` already reports whole-callback health
//! (xruns, worst callback); this narrows that down to a single chain slot.
//!
//! Real-time contract: the audio thread only ever reads one `AtomicBool` and,
//! when profiling is on, stores into preallocated atomics. No allocation, no
//! locks, no syscalls — `Instant::now()` is the same vDSO clock read the I/O
//! callbacks in [`lh_io`] already perform. Profiling is **off by default**, so
//! the steady-state cost is a single relaxed load per block.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::MAX_SLOTS;

/// Exponential moving average weight: `avg = (avg * 7 + sample) / 8`. A power
/// of two so the audio thread does a shift, not a divide.
const EWMA_SHIFT: u32 = 3;

/// Load at or above this fraction of the block budget is a warning.
pub const LOAD_WARNING: f32 = 80.0;
/// Load at or above this fraction of the block budget will glitch.
pub const LOAD_CRITICAL: f32 = 100.0;
/// A single slot consuming this share of the budget is flagged.
pub const SLOT_OVERLOADED: f32 = 25.0;

/// How healthy the chain's timing is relative to its real-time budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    /// Comfortably inside the budget.
    Ok,
    /// At or past [`LOAD_WARNING`] — headroom is thin.
    Warning,
    /// At or past [`LOAD_CRITICAL`] — the deadline is being missed.
    Critical,
}

impl Load {
    fn from_percent(percent: f32) -> Self {
        if percent >= LOAD_CRITICAL {
            Load::Critical
        } else if percent >= LOAD_WARNING {
            Load::Warning
        } else {
            Load::Ok
        }
    }

    /// Text label — never colour alone, so the CLI and a screen reader agree.
    pub fn label(self) -> &'static str {
        match self {
            Load::Ok => "OK",
            Load::Warning => "WARNING",
            Load::Critical => "CRITICAL",
        }
    }
}

/// Per-slot timing written by the audio thread, read by anyone.
///
/// Every counter is a plain atomic over a fixed-size table — sized by
/// [`MAX_SLOTS`], so it neither allocates nor resizes when the chain changes.
#[derive(Debug)]
pub struct SlotProfiler {
    enabled: AtomicBool,
    last_nanos: [AtomicU64; MAX_SLOTS],
    avg_nanos: [AtomicU64; MAX_SLOTS],
    peak_nanos: [AtomicU64; MAX_SLOTS],
    block_last_nanos: AtomicU64,
    block_peak_nanos: AtomicU64,
    budget_nanos: AtomicU64,
    blocks: AtomicU64,
    deadline_misses: AtomicU64,
}

impl Default for SlotProfiler {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            last_nanos: std::array::from_fn(|_| AtomicU64::new(0)),
            avg_nanos: std::array::from_fn(|_| AtomicU64::new(0)),
            peak_nanos: std::array::from_fn(|_| AtomicU64::new(0)),
            block_last_nanos: AtomicU64::new(0),
            block_peak_nanos: AtomicU64::new(0),
            budget_nanos: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
        }
    }
}

impl SlotProfiler {
    /// Turn measurement on or off. Off is the default and costs the audio
    /// thread one relaxed load per block.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Whether the audio thread is currently measuring.
    ///
    /// Called once per block on the audio thread — cheap by construction.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Publish one block's measurements.
    ///
    /// `slot_nanos` is indexed by slot, matching `Chain::slots`; entries for
    /// slots that did not run this block are zero, which correctly decays
    /// their average toward "costs nothing right now". `peak` is worst-ever
    /// and only [`reset`](Self::reset) clears it.
    ///
    /// Audio-thread only. Stores into preallocated atomics; never allocates.
    #[inline]
    pub fn record_block(
        &self,
        slot_nanos: &[u64; MAX_SLOTS],
        total_nanos: u64,
        frames: usize,
        sample_rate: u32,
    ) {
        for (i, &ns) in slot_nanos.iter().enumerate() {
            self.last_nanos[i].store(ns, Ordering::Relaxed);
            self.peak_nanos[i].fetch_max(ns, Ordering::Relaxed);
            let prev = self.avg_nanos[i].load(Ordering::Relaxed);
            // Seed on the first observation so the average does not crawl up
            // from zero over the first several blocks.
            let next = if prev == 0 {
                ns
            } else {
                ((prev << EWMA_SHIFT) - prev + ns) >> EWMA_SHIFT
            };
            self.avg_nanos[i].store(next, Ordering::Relaxed);
        }

        self.block_last_nanos.store(total_nanos, Ordering::Relaxed);
        self.block_peak_nanos
            .fetch_max(total_nanos, Ordering::Relaxed);
        self.blocks.fetch_add(1, Ordering::Relaxed);

        let budget = block_budget_nanos(frames, sample_rate);
        self.budget_nanos.store(budget, Ordering::Relaxed);
        if budget > 0 && total_nanos > budget {
            self.deadline_misses.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Clear every counter, including worst-ever peaks.
    pub fn reset(&self) {
        for i in 0..MAX_SLOTS {
            self.last_nanos[i].store(0, Ordering::Relaxed);
            self.avg_nanos[i].store(0, Ordering::Relaxed);
            self.peak_nanos[i].store(0, Ordering::Relaxed);
        }
        self.block_last_nanos.store(0, Ordering::Relaxed);
        self.block_peak_nanos.store(0, Ordering::Relaxed);
        self.blocks.store(0, Ordering::Relaxed);
        self.deadline_misses.store(0, Ordering::Relaxed);
    }

    /// A consistent-enough copy for display. Counters are read independently,
    /// so a snapshot may straddle a block boundary — fine for a meter, and it
    /// keeps the audio thread free of any synchronisation.
    pub fn snapshot(&self) -> ChainProfile {
        let budget = self.budget_nanos.load(Ordering::Relaxed);
        let pct = |ns: u64| {
            if budget == 0 {
                0.0
            } else {
                ns as f32 / budget as f32 * 100.0
            }
        };
        let block_last = self.block_last_nanos.load(Ordering::Relaxed);

        ChainProfile {
            enabled: self.is_enabled(),
            blocks: self.blocks.load(Ordering::Relaxed),
            budget_nanos: budget,
            block_last_nanos: block_last,
            block_peak_nanos: self.block_peak_nanos.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
            load_percent: pct(block_last),
            slots: std::array::from_fn(|i| {
                let last = self.last_nanos[i].load(Ordering::Relaxed);
                SlotTiming {
                    slot: i,
                    last_nanos: last,
                    avg_nanos: self.avg_nanos[i].load(Ordering::Relaxed),
                    peak_nanos: self.peak_nanos[i].load(Ordering::Relaxed),
                    budget_percent: pct(last),
                }
            }),
        }
    }
}

/// Nanoseconds of wall clock one block of `frames` is allowed to take.
///
/// 64 frames at 48 kHz is 1,333,333 ns — the deadline the white paper sets.
pub fn block_budget_nanos(frames: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    (frames as u64) * 1_000_000_000 / (sample_rate as u64)
}

/// One slot's timing in a [`ChainProfile`].
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SlotTiming {
    /// Index into the chain's slot table.
    pub slot: usize,
    pub last_nanos: u64,
    pub avg_nanos: u64,
    pub peak_nanos: u64,
    /// `last_nanos` as a share of the block budget.
    pub budget_percent: f32,
}

impl SlotTiming {
    /// Whether this one slot is eating [`SLOT_OVERLOADED`] or more of the budget.
    pub fn is_overloaded(&self) -> bool {
        self.budget_percent >= SLOT_OVERLOADED
    }
}

/// A point-in-time copy of [`SlotProfiler`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainProfile {
    pub enabled: bool,
    pub blocks: u64,
    /// Wall clock one block is allowed to take; 0 until the first block lands.
    pub budget_nanos: u64,
    pub block_last_nanos: u64,
    pub block_peak_nanos: u64,
    /// Blocks whose processing exceeded [`budget_nanos`](Self::budget_nanos).
    pub deadline_misses: u64,
    /// Last block's duration as a share of the budget.
    pub load_percent: f32,
    pub slots: [SlotTiming; MAX_SLOTS],
}

impl ChainProfile {
    /// Health of the last block against its budget.
    pub fn load(&self) -> Load {
        Load::from_percent(self.load_percent)
    }

    /// Slots that ran this block, worst first. Allocates — callers are
    /// reporting threads (CLI, GUI), never the audio thread.
    pub fn hot_slots(&self) -> Vec<SlotTiming> {
        let mut hot: Vec<SlotTiming> = self
            .slots
            .iter()
            .copied()
            .filter(|s| s.last_nanos > 0)
            .collect();
        hot.sort_by_key(|s| std::cmp::Reverse(s.last_nanos));
        hot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nanos(slots: &[(usize, u64)]) -> [u64; MAX_SLOTS] {
        let mut a = [0u64; MAX_SLOTS];
        for &(i, ns) in slots {
            a[i] = ns;
        }
        a
    }

    #[test]
    fn budget_follows_frames_and_rate() {
        // The white paper's canonical block: 64 frames at 48 kHz.
        assert_eq!(block_budget_nanos(64, 48_000), 1_333_333);
        // Halving the rate doubles the time available for the same frames.
        assert_eq!(block_budget_nanos(64, 24_000), 2_666_666);
        // A dead stream must not divide by zero.
        assert_eq!(block_budget_nanos(64, 0), 0);
    }

    #[test]
    fn profiling_is_off_until_asked() {
        let p = SlotProfiler::default();
        assert!(!p.is_enabled(), "must not measure unless switched on");
        p.set_enabled(true);
        assert!(p.is_enabled());
    }

    #[test]
    fn peak_holds_and_last_tracks() {
        let p = SlotProfiler::default();
        p.record_block(&nanos(&[(2, 500)]), 900, 64, 48_000);
        p.record_block(&nanos(&[(2, 100)]), 400, 64, 48_000);

        let snap = p.snapshot();
        assert_eq!(
            snap.slots[2].last_nanos, 100,
            "last follows the newest block"
        );
        assert_eq!(snap.slots[2].peak_nanos, 500, "peak holds the worst seen");
        assert_eq!(snap.block_peak_nanos, 900);
        assert_eq!(snap.blocks, 2);
    }

    #[test]
    fn average_seeds_then_converges() {
        let p = SlotProfiler::default();
        // First observation seeds directly rather than crawling up from zero.
        p.record_block(&nanos(&[(0, 800)]), 800, 64, 48_000);
        assert_eq!(p.snapshot().slots[0].avg_nanos, 800);

        // A sustained lower cost pulls the average down toward it.
        for _ in 0..64 {
            p.record_block(&nanos(&[(0, 100)]), 100, 64, 48_000);
        }
        let avg = p.snapshot().slots[0].avg_nanos;
        assert!(
            (95..=105).contains(&avg),
            "average should converge to ~100, got {avg}"
        );
    }

    #[test]
    fn deadline_miss_counts_only_over_budget_blocks() {
        let p = SlotProfiler::default();
        let budget = block_budget_nanos(64, 48_000);

        p.record_block(&nanos(&[]), budget / 2, 64, 48_000);
        assert_eq!(
            p.snapshot().deadline_misses,
            0,
            "inside budget is not a miss"
        );

        p.record_block(&nanos(&[]), budget + 1, 64, 48_000);
        assert_eq!(p.snapshot().deadline_misses, 1, "over budget is a miss");

        // Exactly on budget is not yet a miss — the block still fit.
        p.record_block(&nanos(&[]), budget, 64, 48_000);
        assert_eq!(p.snapshot().deadline_misses, 1);
    }

    #[test]
    fn load_thresholds_bracket_the_budget() {
        let budget = block_budget_nanos(64, 48_000);
        let p = SlotProfiler::default();

        p.record_block(&nanos(&[]), budget / 2, 64, 48_000);
        assert_eq!(p.snapshot().load(), Load::Ok);

        p.record_block(&nanos(&[]), budget * 85 / 100, 64, 48_000);
        assert_eq!(p.snapshot().load(), Load::Warning);

        p.record_block(&nanos(&[]), budget * 2, 64, 48_000);
        assert_eq!(p.snapshot().load(), Load::Critical);
        assert_eq!(Load::Critical.label(), "CRITICAL");
    }

    #[test]
    fn hot_slots_rank_worst_first_and_skip_idle_slots() {
        let p = SlotProfiler::default();
        p.record_block(&nanos(&[(1, 200), (4, 900), (7, 50)]), 1_150, 64, 48_000);

        let hot = p.snapshot().hot_slots();
        assert_eq!(
            hot.iter().map(|s| s.slot).collect::<Vec<_>>(),
            vec![4, 1, 7],
            "ranked by cost, idle slots omitted"
        );
    }

    #[test]
    fn a_slot_eating_a_quarter_of_the_budget_is_flagged() {
        let p = SlotProfiler::default();
        let budget = block_budget_nanos(64, 48_000);
        p.record_block(
            &nanos(&[(3, budget / 2), (5, budget / 100)]),
            budget,
            64,
            48_000,
        );

        let snap = p.snapshot();
        assert!(
            snap.slots[3].is_overloaded(),
            "half the budget is an overload"
        );
        assert!(!snap.slots[5].is_overloaded(), "1% is not");
    }

    #[test]
    fn reset_clears_peaks_and_counters() {
        let p = SlotProfiler::default();
        p.record_block(&nanos(&[(0, 500)]), 5_000_000, 64, 48_000);
        p.reset();

        let snap = p.snapshot();
        assert_eq!(snap.slots[0].peak_nanos, 0);
        assert_eq!(snap.block_peak_nanos, 0);
        assert_eq!(snap.blocks, 0);
        assert_eq!(snap.deadline_misses, 0);
    }
}
