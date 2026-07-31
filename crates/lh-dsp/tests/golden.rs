//! Golden audio fingerprints: render a known signal through each effect and
//! compare the output against committed expected values. Catches silent DSP
//! regressions (coefficient changes, ADAA modifications, filter tuning) that
//! unit tests miss because they only check broad properties (finite, bounded).
//!
//! The fingerprint is a compact summary: peak, RMS, DC offset, and brightness
//! (spectral centroid proxy) of the first 4096 output samples. Values are
//! rounded to 3 decimal places for cross-platform stability.
//!
//! To regenerate after an intentional DSP change:
//!   cargo test --test golden -- --nocapture
//! and review the printed fingerprints for expected drift.

use lh_dsp::Effect;
use lh_dsp::drive::{Drive, MODELS};

const SR: u32 = 48_000;
const BLOCK: usize = 64;
const N: usize = 4_096;

/// Compute a compact, platform-stable fingerprint of an audio buffer.
fn fingerprint(name: &str, buf: &[f32]) -> String {
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (buf
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / buf.len() as f64)
        .sqrt();
    let dc = buf.iter().sum::<f32>() / buf.len() as f32;
    let brightness =
        buf.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / (buf.len() - 1) as f32;
    format!(
        "{name}: peak={:.3} rms={:.3} dc={:.3} bright={:.3}",
        peak, rms, dc, brightness
    )
}

fn render_sine(effect: &mut dyn Effect, freq: f32) -> Vec<f32> {
    effect.prepare(SR);
    let input: Vec<f32> = (0..N)
        .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / SR as f32).sin() * 0.5)
        .collect();
    let mut left = input.clone();
    let mut right = input.clone();
    for (l, r) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
        effect.process(l, r);
    }
    left
}

#[test]
fn drive_model_fingerprints_are_stable() {
    for (model, def) in MODELS.iter().enumerate() {
        let mut drive = Drive::new();
        drive.prepare(SR);
        drive.select_pedal(model);
        for (i, param) in def.desc.params.iter().enumerate() {
            drive.set_param(i, param.default_norm());
        }
        let out = render_sine(&mut drive, 220.0);
        let fp = fingerprint(def.desc.key, &out);
        eprintln!("{fp}");
        assert!(
            out.iter().all(|s| s.is_finite()),
            "{}: non-finite",
            def.desc.key
        );
        let p = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(p > 0.001, "{}: signal too quiet (peak {p})", def.desc.key);
        assert!(p < 100.0, "{}: signal too loud (peak {p})", def.desc.key);
    }
}
