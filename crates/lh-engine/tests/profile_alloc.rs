//! The DSP profiler must not break the real-time contract.
//!
//! Instrumentation is the classic way an audio thread starts allocating: a
//! `Vec` of samples here, a formatted label there, and the callback that used
//! to be lock-free is suddenly calling `malloc` under the user's fingers.
//!
//! This binary proves it does not happen. It drives the same `Chain::process`
//! path the audio callback uses, **with profiling switched on**, under the same
//! allocator guard `lh-dsp` uses for its pedals — including the cases that
//! actually tempt an allocation: a chunked block (so per-slot costs accumulate
//! across several chunks), and a mid-crossfade bypass toggle (the branch that
//! touches the dry-copy scratch buffers).
//!
//! A **separate test binary** on purpose: `#[global_allocator]` is crate-wide,
//! and the engine's other tests have no business running under one.
//!
//! **Debug builds only**, matching every other `assert_no_alloc` site in this
//! workspace. In release the sweep still runs and still checks the audio stays
//! finite, but nothing watches the allocator; `cargo test` is the real gate.

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Run `f` under the allocation guard where the guard exists.
#[cfg(debug_assertions)]
fn guarded<R>(f: impl FnOnce() -> R) -> R {
    assert_no_alloc::assert_no_alloc(f)
}
#[cfg(not(debug_assertions))]
fn guarded<R>(f: impl FnOnce() -> R) -> R {
    f()
}

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use lh_dsp::dynamics::NoiseGate;
use lh_dsp::time::Delay;
use lh_engine::build_chain;

const SR: u32 = 48_000;

fn pedalboard() -> Vec<Box<dyn Effect>> {
    vec![
        Box::new(NoiseGate::new()),
        Box::new(Drive::new()),
        Box::new(Delay::new()),
    ]
}

/// A block larger than `MAX_BLOCK`, so `process` chunks it internally and the
/// profiler accumulates several partial costs per slot before publishing.
const LONG_BLOCK: usize = 4_096;

#[test]
fn profiling_the_chain_never_allocates() {
    let (mut chain, handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);

    let mut left = vec![0.0f32; LONG_BLOCK];
    let mut right = vec![0.0f32; LONG_BLOCK];
    for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
        let s = (i as f32 * 0.01).sin() * 0.5;
        *l = s;
        *r = s;
    }

    guarded(|| {
        for _ in 0..16 {
            chain.process(&mut left, &mut right);
        }
    });

    assert!(
        left.iter().chain(right.iter()).all(|s| s.is_finite()),
        "profiling must not disturb the audio"
    );

    let snap = handle.telemetry().profile().snapshot();
    assert!(snap.enabled);
    assert_eq!(snap.blocks, 16, "every block should have been recorded");
    assert!(
        snap.block_last_nanos > 0,
        "a real block must take measurable time"
    );
    assert!(
        snap.slots.iter().any(|s| s.last_nanos > 0),
        "at least one slot must report a cost"
    );
}

#[test]
fn profiling_during_a_bypass_crossfade_never_allocates() {
    // The mid-crossfade branch copies into the dry scratch buffers — a
    // different path from the settled fast path, and the one most likely to
    // reach for memory.
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);

    let drive = handle.order_handles()[1].clone();
    let mut left = vec![0.1f32; 256];
    let mut right = vec![0.1f32; 256];

    guarded(|| {
        for i in 0..32 {
            // Toggle bypass every few blocks so the wet smoother is mid-flight
            // while the profiler is timing the slot.
            if i % 4 == 0 {
                let _ = handle.set_active(&drive, i % 8 == 0);
            }
            chain.process(&mut left, &mut right);
        }
    });

    assert!(
        left.iter().chain(right.iter()).all(|s| s.is_finite()),
        "crossfade under profiling must stay finite"
    );
}

#[test]
fn disabled_profiling_records_nothing_and_still_never_allocates() {
    let (mut chain, handle) = build_chain(pedalboard());
    chain.prepare(SR);
    // Deliberately left off — the default.

    let mut left = vec![0.05f32; 256];
    let mut right = vec![0.05f32; 256];

    guarded(|| {
        for _ in 0..8 {
            chain.process(&mut left, &mut right);
        }
    });

    let snap = handle.telemetry().profile().snapshot();
    assert!(!snap.enabled);
    assert_eq!(snap.blocks, 0, "a disabled profiler must stay silent");
    assert_eq!(snap.block_last_nanos, 0);
    assert!(snap.slots.iter().all(|s| s.last_nanos == 0));
}
