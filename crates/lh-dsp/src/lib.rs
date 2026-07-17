//! Hand-written effects for Lion-Heart.
//!
//! Every effect implements [`Effect`]: pure buffer-in/buffer-out mono
//! processing that runs offline (tests, benches) exactly as it runs on the
//! audio thread. Real-time rules apply to `reset`, `set_param`, and `process`
//! (no allocation, no locks, no syscalls); `prepare` is the one place allowed
//! to allocate.

pub mod biquad;
pub mod cab;
pub mod comp;
pub mod delay;
pub mod drive;
pub mod eq;
pub mod gate;
pub mod limiter;
pub mod modulation;
pub mod oversample;
pub mod reverb;
pub mod smooth;
pub mod swap;
pub mod testutil;
pub mod tuner;

use lh_core::{EffectDesc, FamilyDesc};

pub trait Effect: Send {
    /// The family this effect fills: one chain slot, 1..N selectable pedals
    /// (PRD 001). Single-pedal effects list themselves as the only pedal.
    fn family(&self) -> &'static FamilyDesc;

    /// Index of the active pedal within the family.
    fn pedal_index(&self) -> usize {
        0
    }

    /// Switch pedals. RT-safe: every pedal is preallocated at construction;
    /// switching is an index change plus a state reset of the incoming pedal
    /// (a brief discontinuity, never an allocation, never NaN). Same-pedal
    /// and out-of-range selections are ignored. Values are *not* carried
    /// over — the control side re-sends the incoming pedal's params (its
    /// shadow is the per-pedal value memory).
    fn select_pedal(&mut self, _pedal: usize) {}

    /// The active pedal's descriptor — the current param index space.
    fn descriptor(&self) -> &'static EffectDesc {
        self.family().pedals[self.pedal_index()]
    }

    /// Configure for a sample rate and allocate internal buffers.
    /// Called off the audio thread before processing starts. Must also snap
    /// all smoothers so processing starts from the current targets.
    fn prepare(&mut self, sample_rate: u32);

    /// Clear runtime state (delay lines, envelopes, filter memories). RT-safe.
    fn reset(&mut self);

    /// Set a parameter of the **active pedal** to a normalized `0..=1`
    /// value. RT-safe: stores a smoother target; the audible change happens
    /// inside `process`. Out-of-range indices are ignored.
    fn set_param(&mut self, index: usize, normalized: f32);

    /// In-place stereo processing of one block, any length. RT-safe.
    /// `left` and `right` are always the same length (the engine guarantees
    /// it); a mono source enters the chain duplicated onto both channels.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
}
