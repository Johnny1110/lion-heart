//! Property-based tests for DSP invariants.
//!
//! These use `proptest` to generate random parameter vectors and verify
//! invariants that hold for every effect:
//!
//! - **Boundedness**: output is finite and bounded for any parameter vector
//! - **Block partition invariance**: processing at block size 32 vs 1024
//!   gives equivalent output (within float tolerance)
//! - **Silence in, silence out**: feeding silence with any params produces
//!   silence (or at most a decaying tail from filter state)

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use proptest::prelude::*;

const SR: u32 = 48_000;
const N: usize = 1_024;

fn sine(freq: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin() * 0.5)
        .collect()
}

fn process_at(effect: &mut dyn Effect, input: &[f32], block: usize) -> Vec<f32> {
    let mut left = input.to_vec();
    let mut right = input.to_vec();
    for (l, r) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
        effect.process(l, r);
    }
    left
}

/// Any parameter vector (normalized 0..=1) — proptest generates these.
fn any_params(count: usize) -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(0.0f32..1.0, count)
}

proptest! {
    /// Output must be finite and bounded for any drive parameter vector.
    #[test]
    fn drive_output_is_finite_and_bounded(params in any_params(6)) {
        let mut drive = Drive::new();
        drive.prepare(SR);
        for (i, &p) in params.iter().enumerate() {
            drive.set_param(i, p);
        }
        let input = sine(220.0, N);
        let out = process_at(&mut drive, &input, 64);
        for s in &out {
            prop_assert!(s.is_finite(), "non-finite sample: {s}");
        }
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        prop_assert!(peak < 1e6, "output too loud: {peak}");
    }

    /// Block partition invariance: processing at 32 vs 1024 should be
    /// equivalent (within float rounding). This catches block-boundary
    /// state bugs.
    #[test]
    fn drive_block_partition_invariant(params in any_params(6)) {
        let mut drive_a = Drive::new();
        drive_a.prepare(SR);
        for (i, &p) in params.iter().enumerate() {
            drive_a.set_param(i, p);
        }
        let input = sine(440.0, N);
        let out_32 = process_at(&mut drive_a, &input, 32);

        let mut drive_b = Drive::new();
        drive_b.prepare(SR);
        for (i, &p) in params.iter().enumerate() {
            drive_b.set_param(i, p);
        }
        let out_1024 = process_at(&mut drive_b, &input, 1024);

        // Skip the first few samples (smoother transient differences).
        let skip = 64;
        for (a, b) in out_32[skip..].iter().zip(&out_1024[skip..]) {
            let diff = (a - b).abs();
            prop_assert!(diff < 1e-2, "block partition mismatch: {diff} at {a} vs {b}");
        }
    }

    /// Silence in, silence out (or very quiet): feeding silence with any
    /// params should not produce a loud output.
    #[test]
    fn drive_silence_produces_near_silence(params in any_params(6)) {
        let mut drive = Drive::new();
        drive.prepare(SR);
        for (i, &p) in params.iter().enumerate() {
            drive.set_param(i, p);
        }
        let input = vec![0.0f32; N];
        let out = process_at(&mut drive, &input, 64);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        prop_assert!(peak < 0.1, "silence input produced loud output: {peak}");
    }
}
