//! Hand-written effects for Lion-Heart.
//!
//! Every effect implements [`Effect`]: pure buffer-in/buffer-out mono
//! processing that runs offline (tests, benches) exactly as it runs on the
//! audio thread. Real-time rules apply to `reset`, `set_param`, and `process`
//! (no allocation, no locks, no syscalls); `prepare` is the one place allowed
//! to allocate.

pub mod delay;
pub mod drive;
pub mod gate;
pub mod oversample;
pub mod smooth;
pub mod testutil;

use lh_core::EffectDesc;

pub trait Effect: Send {
    fn descriptor(&self) -> &'static EffectDesc;

    /// Configure for a sample rate and allocate internal buffers.
    /// Called off the audio thread before processing starts. Must also snap
    /// all smoothers so processing starts from the current targets.
    fn prepare(&mut self, sample_rate: u32);

    /// Clear runtime state (delay lines, envelopes, filter memories). RT-safe.
    fn reset(&mut self);

    /// Set a parameter to a normalized `0..=1` value. RT-safe: stores a
    /// smoother target; the audible change happens inside `process`.
    fn set_param(&mut self, index: usize, normalized: f32);

    /// In-place mono processing of one block, any length. RT-safe.
    fn process(&mut self, block: &mut [f32]);
}
