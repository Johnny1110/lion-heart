//! Session configuration: `AppConfig`, `SessionOpts`, and config I/O.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::practice::{GrooveSnapshot, MetroSnapshot};

/// App-global configuration persisted to `~/.lion-heart/config.json`.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub last_preset: Option<String>,
    /// Last directories a NAM / IR was loaded from (browser starting points).
    #[serde(default)]
    pub nam_dir: Option<String>,
    #[serde(default)]
    pub ir_dir: Option<String>,
    /// Last directory a backing track was loaded from (PRD 019 Phase 3).
    #[serde(default)]
    pub song_dir: Option<String>,
    /// Audio I/O applied from the GUI settings panel; used when the matching
    /// CLI flag is absent. `buffer` stores the requested frames, 0 = device
    /// default; absent fields fall back to the app defaults.
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub buffer: Option<u32>,
    #[serde(default)]
    pub in_channel: Option<u16>,
    /// Snapshot morph time in milliseconds (PRD 009): 0 = instant switch
    /// (the effects' own smoothing declicks it), up to 2000 for an audible
    /// scene sweep. App-global — one glide feel for the rig.
    #[serde(default)]
    pub morph_ms: u32,
    /// Spillover (PRD 010): let delay/reverb tails ring out after a preset
    /// switch or slot removal instead of being cut. On by default.
    #[serde(default = "spillover_default")]
    pub spillover: bool,
    /// Global tempo in BPM (ADR 014): drives every effect whose `sync`
    /// selector is locked to a note division. App-global — one tempo for the
    /// rig, like `morph_ms`. Persisted so the tapped/typed tempo survives a
    /// restart.
    #[serde(default = "tempo_default")]
    pub tempo_bpm: f32,
    /// Where recordings land (PRD 014). Absent = `~/.lion-heart/recordings`.
    /// App-global — a monitor recorder, not part of any preset.
    #[serde(default)]
    pub recordings_dir: Option<String>,
    /// Recording bit depth: 16, 24, or 32 (32 = IEEE float). Default 24 —
    /// plenty of headroom at half the size of float.
    #[serde(default = "record_bits_default")]
    pub record_bits: u16,
}

fn spillover_default() -> bool {
    true
}

fn tempo_default() -> f32 {
    lh_core::tempo::DEFAULT_BPM
}

fn record_bits_default() -> u16 {
    24
}

impl Default for AppConfig {
    /// Matches the serde field defaults — notably `spillover: true`, so a
    /// fresh config (no file) and a file missing the field agree.
    fn default() -> Self {
        Self {
            last_preset: None,
            nam_dir: None,
            ir_dir: None,
            song_dir: None,
            input: None,
            output: None,
            buffer: None,
            in_channel: None,
            morph_ms: 0,
            spillover: spillover_default(),
            tempo_bpm: tempo_default(),
            recordings_dir: None,
            record_bits: record_bits_default(),
        }
    }
}

/// Runtime options passed to `Session::start`.
#[derive(Clone)]
pub struct SessionOpts {
    pub input: Option<String>,
    pub output: Option<String>,
    pub sample_rate: u32,
    pub buffer: Option<u32>,
    pub in_channel: u16,
    pub gain_db: f32,
    pub prefill_blocks: u32,
    /// Install the raw-input tap for the tuner (GUI).
    pub tuner_tap: bool,
    /// Install the post-output tap for the spectrum analyzer (GUI).
    pub spectrum_tap: bool,
    /// MIDI input port override (name substring or index); `None` follows
    /// `midi.json` / first available port.
    pub midi_port: Option<String>,
}

/// Chain and asset state that survives an audio-engine restart
/// (`Session::carry_over` → `Session::resume`).
pub struct CarryOver {
    pub(super) chain: Vec<lh_core::preset::SlotState>,
    pub(super) nam: Option<lh_core::preset::AssetRef>,
    pub(super) ir: Option<lh_core::preset::AssetRef>,
    pub(super) ir_b: Option<lh_core::preset::AssetRef>,
    pub(super) snapshots: std::collections::BTreeMap<String, lh_core::preset::Snapshot>,
    pub(super) active_snapshot: Option<String>,
    /// Metronome runtime state (PRD 019), so a device restart keeps the click.
    pub(super) metronome: MetroSnapshot,
    /// Drum-groove runtime state (PRD 019 Phase 2), same reason.
    pub(super) groove: GrooveSnapshot,
}

/// Read `~/.lion-heart/config.json` (defaults when absent).
pub fn load_config() -> AppConfig {
    lh_assets::app_dir()
        .map(|d| d.join("config.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Where takes are written (PRD 014): the configured directory, else
/// `~/.lion-heart/recordings`, else the current dir if the home directory is
/// unavailable.
pub(crate) fn recordings_dir(config: &AppConfig) -> PathBuf {
    if let Some(dir) = &config.recordings_dir {
        return PathBuf::from(dir);
    }
    lh_assets::app_dir()
        .map(|d| d.join("recordings"))
        .unwrap_or_else(|| PathBuf::from("recordings"))
}

/// Write `~/.lion-heart/config.json`.
pub(crate) fn save_config(config: &AppConfig) {
    let Some(dir) = lh_assets::app_dir() else {
        return;
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("config.json"),
            serde_json::to_string_pretty(config).expect("config serializes"),
        )
    };
    if let Err(e) = write() {
        eprintln!("warning: could not save config: {e}");
    }
}
