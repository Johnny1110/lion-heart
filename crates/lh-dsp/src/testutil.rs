//! Offline render harness: run effects without any audio device, in tests and
//! benches, exactly as they run on the audio thread.

use crate::Effect;

pub fn sine(sample_rate: u32, freq: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / sample_rate as f32).sin())
        .collect()
}

pub fn silence(len: usize) -> Vec<f32> {
    vec![0.0; len]
}

pub fn impulse(len: usize, at: usize) -> Vec<f32> {
    let mut v = vec![0.0; len];
    v[at] = 1.0;
    v
}

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Panics with `name` if any sample is NaN/inf — every effect test's baseline.
pub fn assert_finite(name: &str, x: &[f32]) {
    for (i, s) in x.iter().enumerate() {
        assert!(s.is_finite(), "{name}: non-finite sample {s} at index {i}");
    }
}

/// Render `input` through `effect` in fixed-size blocks, like the engine does.
pub fn process_in_blocks(effect: &mut dyn Effect, input: &[f32], block: usize) -> Vec<f32> {
    let mut out = input.to_vec();
    for chunk in out.chunks_mut(block.max(1)) {
        effect.process(chunk);
    }
    out
}
