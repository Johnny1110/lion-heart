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

// ---------------------------------------------------------------------------
// P1-8: allocation-free guarantee for every effect family, not just Drive.
// ---------------------------------------------------------------------------

use lh_dsp::cab::CabIr;
use lh_dsp::dynamics::comp::FAMILY as COMP_FAMILY;
use lh_dsp::dynamics::gate::FAMILY as GATE_FAMILY;
use lh_dsp::dynamics::limiter::FAMILY as LIM_FAMILY;
use lh_dsp::dynamics::{Compressor, Limiter, NoiseGate};
use lh_dsp::eq::Eq;
use lh_dsp::filter::Filter;
use lh_dsp::modulation::{FAMILY as MOD_FAMILY, Modulation};
use lh_dsp::power::PowerAmp;
use lh_dsp::time::Delay;
use lh_dsp::time::delay::FAMILY as DELAY_FAMILY;
use lh_dsp::time::reverb::{FAMILY as REVERB_FAMILY, Reverb, VOICE_COUNT as REVERB_VOICES};

/// Generic hammer: prepare the effect, then sweep every param while
/// processing hot and cold blocks inside the no-allocation guard.
fn hammer_effect(mut effect: Box<dyn Effect>, params: usize) {
    effect.prepare(SR);
    let mut left = vec![0.0f32; BLOCK];
    let mut right = vec![0.0f32; BLOCK];
    guarded(|| {
        for n in 0..64 {
            for p in 0..params {
                let t = ((n * 7 + p * 13) % 64) as f32 / 63.0;
                effect.set_param(p, t);
            }
            let amp = if n % 8 == 7 { 1e5 } else { 0.2 };
            for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                let ph = std::f32::consts::TAU * 220.0 * i as f32 / SR as f32;
                *l = amp * ph.sin();
                *r = amp * ph.cos();
            }
            effect.process(&mut left, &mut right);
            for s in left.iter().chain(right.iter()) {
                assert!(s.is_finite(), "effect produced non-finite sample at n={n}");
            }
        }
    });
}

#[test]
fn modulation_voices_are_allocation_free() {
    for voice in 0..MOD_FAMILY.pedals.len() {
        let mut modul = Modulation::new();
        modul.select_pedal(voice);
        hammer_effect(Box::new(modul), MOD_FAMILY.pedals[voice].params.len());
    }
}

#[test]
fn delay_voices_are_allocation_free() {
    for voice in 0..DELAY_FAMILY.pedals.len() {
        let mut delay = Delay::new();
        delay.select_pedal(voice);
        hammer_effect(Box::new(delay), DELAY_FAMILY.pedals[voice].params.len());
    }
}

#[test]
fn reverb_voices_are_allocation_free() {
    for voice in 0..REVERB_VOICES {
        let mut rev = Reverb::new();
        rev.select_pedal(voice);
        hammer_effect(Box::new(rev), REVERB_FAMILY.pedals[voice].params.len());
    }
}

#[test]
fn eq_is_allocation_free() {
    hammer_effect(
        Box::new(Eq::new()),
        lh_dsp::eq::FAMILY.pedals[0].params.len(),
    );
}

#[test]
fn filter_is_allocation_free() {
    hammer_effect(
        Box::new(Filter::new()),
        lh_dsp::filter::FAMILY.pedals[0].params.len(),
    );
}

#[test]
fn compressor_is_allocation_free() {
    for voice in 0..COMP_FAMILY.pedals.len() {
        let mut comp = Compressor::new();
        comp.select_pedal(voice);
        hammer_effect(Box::new(comp), COMP_FAMILY.pedals[voice].params.len());
    }
}

#[test]
fn noise_gate_is_allocation_free() {
    hammer_effect(
        Box::new(NoiseGate::new()),
        GATE_FAMILY.pedals[0].params.len(),
    );
}

#[test]
fn limiter_is_allocation_free() {
    hammer_effect(Box::new(Limiter::new()), LIM_FAMILY.pedals[0].params.len());
}

#[test]
fn power_amp_is_allocation_free() {
    hammer_effect(
        Box::new(PowerAmp::new()),
        lh_dsp::power::FAMILY.pedals[0].params.len(),
    );
}

#[test]
fn cab_ir_is_allocation_free_without_asset() {
    // CabIr passes audio through when no IR is installed.
    let (cab, _handle) = CabIr::new();
    hammer_effect(Box::new(cab), lh_dsp::cab::FAMILY.pedals[0].params.len());
}
