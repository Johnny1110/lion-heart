//! Preset loudness leveling (PRD 016): measure how loud each preset renders a
//! reference DI (integrated LUFS, BS.1770 — [`lh_dsp::loudness`]) and store a
//! per-preset **master trim** that brings it to a target loudness, so switching
//! presets live stops jumping in volume.
//!
//! The trim is **app-global environment, not tone**: it lives in
//! `~/.lion-heart/levels.json` (like `global_eq.json`), never in a preset, and
//! is absent from the plugin — the DAW owns loudness there. The measurement
//! reuses the offline [`crate::render`] pipeline (the exact live chain), so a
//! measured preset is levelled against what you actually hear.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lh_assets::app_dir;
use lh_assets::wav::WavData;
use lh_core::preset::Preset;
use lh_dsp::loudness::integrated_lufs;
use serde::{Deserialize, Serialize};

use crate::render::{self, ENGINE_RATE, RenderError, render};

/// Default integrated-loudness target (EBU R128 is −23; guitar rigs sit hotter,
/// and the safety limiter guards the ceiling, so −18 LUFS is the app default).
pub const DEFAULT_TARGET_LUFS: f32 = -18.0;

/// Clamp on an applied trim: a measurement glitch (a near-silent preset) must
/// not command a wild boost. ±12 dB covers real preset-to-preset spread.
pub const MAX_TRIM_DB: f32 = 12.0;

/// Seconds of tail rendered after the reference DI so delay/reverb spill is
/// included in the loudness (the gate drops the quiet part anyway).
const MEASURE_TAIL_SECS: f32 = 1.0;

fn default_target() -> f32 {
    DEFAULT_TARGET_LUFS
}

/// `~/.lion-heart/levels.json` contents: the loudness target and the per-preset
/// trims measured against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    #[serde(default = "default_target")]
    pub target_lufs: f32,
    /// preset name → master trim in dB.
    #[serde(default)]
    pub trims: BTreeMap<String, f32>,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            target_lufs: DEFAULT_TARGET_LUFS,
            trims: BTreeMap::new(),
        }
    }
}

impl Levels {
    pub fn path() -> Option<PathBuf> {
        app_dir().map(|d| d.join("levels.json"))
    }

    /// Read `levels.json` (empty default when absent, warning on bad JSON).
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                eprintln!("warning: {}: {e} — ignoring levels", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(dir) = app_dir() else { return };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("levels.json"),
                serde_json::to_string_pretty(self).expect("levels serialize"),
            )
        };
        if let Err(e) = write() {
            eprintln!("warning: could not save levels: {e}");
        }
    }

    /// The stored trim (dB) for `preset`, or 0 dB if none is recorded.
    pub fn trim_db(&self, preset: &str) -> f32 {
        self.trims.get(preset).copied().unwrap_or(0.0)
    }
}

/// The master trim (dB) that brings `measured_lufs` to `target_lufs`, clamped to
/// [`MAX_TRIM_DB`]. A gated/silent measurement (non-finite) yields 0 dB — leave
/// a preset that measured as silence alone.
pub fn trim_for(measured_lufs: f32, target_lufs: f32) -> f32 {
    if !measured_lufs.is_finite() {
        return 0.0;
    }
    (target_lufs - measured_lufs).clamp(-MAX_TRIM_DB, MAX_TRIM_DB)
}

/// A deterministic, synthesized reference DI (PRD 016): six open-string plucks
/// (E2–E4), each a decaying harmonic stack, at a realistic DI level. Shipping a
/// recorded-guitar binary was rejected (ADR 021 precedent); a synthesized
/// broadband yardstick measures the *relative* preset spread just as well —
/// the same signal goes through every preset. Overridable with `level --ref`.
pub fn reference_di() -> WavData {
    const NOTES: [f32; 6] = [82.41, 110.0, 146.83, 196.0, 246.94, 329.63];
    const NOTE_SECS: f32 = 0.8;
    const DECAY_TAU: f32 = 0.35;
    let sr = ENGINE_RATE;
    let note_len = (NOTE_SECS * sr as f32) as usize;
    let mut samples = Vec::with_capacity(note_len * NOTES.len());
    for &f0 in &NOTES {
        for n in 0..note_len {
            let t = n as f32 / sr as f32;
            let env = (-t / DECAY_TAU).exp();
            let mut s = 0.0;
            for h in 1..=6u32 {
                s += (1.0 / h as f32) * (std::f32::consts::TAU * f0 * h as f32 * t).sin();
            }
            samples.push(s * env);
        }
    }
    // Normalize to a hot-but-clean DI peak (−8 dBFS) so drives engage as they
    // would on real playing.
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-9);
    let gain = 0.4 / peak;
    for s in &mut samples {
        *s *= gain;
    }
    WavData {
        samples,
        channels: 1,
        sample_rate: sr,
    }
}

/// Render `di` through `preset` (offline, the live chain) and return its
/// integrated loudness in LUFS plus any render warnings (missing/relocated
/// assets skew loudness — the caller should surface them). `preset_dir`
/// resolves the preset's assets.
pub fn measure(
    preset: &Preset,
    di: &WavData,
    preset_dir: Option<&Path>,
) -> Result<(f32, Vec<String>), RenderError> {
    let rendered = render(preset, di, preset_dir, MEASURE_TAIL_SECS)?;
    let frames = rendered.samples.len() / 2;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for f in 0..frames {
        left.push(rendered.samples[2 * f]);
        right.push(rendered.samples[2 * f + 1]);
    }
    Ok((
        integrated_lufs(&left, &right, render::ENGINE_RATE),
        rendered.warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lh_core::preset::{PRESET_SCHEMA_VERSION, Preset, PresetAssets};
    use std::collections::BTreeMap;

    #[test]
    fn trim_moves_measured_to_target() {
        assert!((trim_for(-24.0, -18.0) - 6.0).abs() < 1e-6);
        assert!((trim_for(-12.0, -18.0) - -6.0).abs() < 1e-6);
        // Clamped either way.
        assert_eq!(trim_for(-60.0, -18.0), MAX_TRIM_DB);
        assert_eq!(trim_for(0.0, -18.0), -MAX_TRIM_DB);
        // Silence → leave it alone.
        assert_eq!(trim_for(f32::NEG_INFINITY, -18.0), 0.0);
    }

    #[test]
    fn levels_round_trip() {
        let mut levels = Levels::default();
        levels.trims.insert("lead".into(), -2.3);
        levels.trims.insert("clean".into(), 1.7);
        let json = serde_json::to_string(&levels).unwrap();
        let back: Levels = serde_json::from_str(&json).unwrap();
        assert_eq!(levels, back);
        assert_eq!(back.trim_db("lead"), -2.3);
        assert_eq!(back.trim_db("missing"), 0.0);
        assert_eq!(back.target_lufs, DEFAULT_TARGET_LUFS);
    }

    #[test]
    fn missing_target_defaults() {
        let back: Levels = serde_json::from_str(r#"{"trims":{"a":1.0}}"#).unwrap();
        assert_eq!(back.target_lufs, DEFAULT_TARGET_LUFS);
        assert_eq!(back.trim_db("a"), 1.0);
    }

    #[test]
    fn reference_di_is_finite_and_hot() {
        let di = reference_di();
        assert_eq!(di.channels, 1);
        assert_eq!(di.sample_rate, ENGINE_RATE);
        assert!(di.samples.iter().all(|s| s.is_finite()));
        let peak = di.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.4).abs() < 1e-4,
            "normalized to −8 dBFS, peak {peak}"
        );
    }

    #[test]
    fn measuring_a_passthrough_preset_reads_the_di_loudness() {
        // An empty preset is transparent, so the render measures the reference
        // DI's own loudness — a finite, plausible figure well above the gate.
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: "empty".into(),
            chain: vec![],
            assets: PresetAssets::default(),
            snapshots: BTreeMap::new(),
            active_snapshot: None,
        };
        let (lufs, warnings) = measure(&preset, &reference_di(), None).unwrap();
        assert!(warnings.is_empty(), "no-asset preset: {warnings:?}");
        assert!(
            lufs.is_finite() && (-40.0..0.0).contains(&lufs),
            "reference DI loudness {lufs} LUFS should be sane"
        );
        // And it yields a bounded trim toward the default target.
        let trim = trim_for(lufs, DEFAULT_TARGET_LUFS);
        assert!(trim.abs() <= MAX_TRIM_DB);
    }
}
