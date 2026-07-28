//! Every drive pedal must be allocation-free on the audio thread.
//!
//! Until now this rule (CLAUDE.md real-time rule 1) was only enforced *at
//! runtime*, by the app's debug-build `AllocDisabler` around the audio callback
//! — which means it is enforced when someone plays through the pedal, on
//! hardware, with the right build profile. That is a real gate, but a slow one
//! to reach and easy to skip.
//!
//! This binary brings it offline. It installs the same allocator and runs every
//! registered drive model through the same `process` path the callback uses,
//! doing the things that actually tempt a circuit model into allocating: knobs
//! sweeping (which rebuilds filter coefficients — and, since PRD 026, whole
//! scattering matrices), stepped selectors switching, and the input slammed far
//! past anything a guitar produces.
//!
//! It is a **separate test binary** on purpose. `#[global_allocator]` is
//! crate-wide, and the library's 300-odd unit tests have no business running
//! under one.
//!
//! **Debug builds only**, like every other `assert_no_alloc` site in this
//! workspace: the crate compiles `AllocDisabler` out under `disable_release`, so
//! in release the sweep still runs and still checks the output stays finite, but
//! nothing watches the allocator. `cargo test` is the gate that matters here.

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
use lh_dsp::drive::{Drive, FAMILY, MODELS};

const SR: u32 = 48_000;
const BLOCK: usize = 64;

/// Run one pedal hard, inside the no-allocation section.
///
/// Everything that may allocate — building the effect, the buffers — happens
/// outside it. What runs inside is exactly what the audio callback runs.
fn hammer(model: usize) {
    let mut drive = Drive::new();
    drive.prepare(SR);
    drive.select_pedal(model);

    let params = MODELS[model].desc.params.len();
    let mut left = vec![0.0f32; BLOCK];
    let mut right = vec![0.0f32; BLOCK];

    guarded(|| {
        for n in 0..64 {
            // A knob a human is turning: every param, never twice the same
            // value, so no settled-skip anywhere can hide a rebuild path.
            for p in 0..params {
                let t = ((n * 7 + p * 13) % 64) as f32 / 63.0;
                drive.set_param(p, t);
            }
            // Nominal level for most blocks, then far past full scale — the
            // solvers' cold, stiff corner.
            let amp = if n % 8 == 7 { 1e5 } else { 0.2 };
            for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                let ph = std::f32::consts::TAU * 220.0 * i as f32 / SR as f32;
                *l = amp * ph.sin();
                *r = amp * ph.cos();
            }
            drive.process(&mut left, &mut right);
            for s in left.iter().chain(right.iter()) {
                assert!(s.is_finite(), "{} produced a non-finite sample", model);
            }
        }
    });
}

#[test]
fn every_drive_pedal_is_allocation_free_under_a_knob_sweep() {
    for model in 0..FAMILY.pedals.len() {
        hammer(model);
    }
}
