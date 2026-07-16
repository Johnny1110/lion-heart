//! The engine session shared by the jam REPL and the GUI: builds the
//! pedalboard chain, starts the duplex stream, loads assets, and persists
//! presets/config under `~/.lion-heart/`.
//!
//! Feedback discipline: operations return message strings instead of
//! printing, so the REPL can `println!` them and the GUI can show them in a
//! status line. `Err` is a single user-facing error message.

use std::path::{Path, PathBuf};

use lh_core::preset::{AssetRef, PRESET_SCHEMA_VERSION, Preset, PresetAssets};
use lh_dsp::Effect;
use lh_dsp::cab::{CabIr, IrAsset};
use lh_dsp::comp::Compressor;
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::eq::Eq;
use lh_dsp::gate::NoiseGate;
use lh_dsp::limiter::Limiter;
use lh_dsp::modulation::Modulation;
use lh_dsp::reverb::Reverb;
use lh_dsp::swap::AssetHandle;
use lh_engine::{ChainHandle, build_chain};
use lh_io::passthrough::{DuplexRunner, RunnerOpts};
use lh_io::stats::Snapshot;
use lh_nam::{NamAmp, NamAsset, load_nam_file};
use serde::{Deserialize, Serialize};

/// Samples of raw input buffered for the tuner (~85 ms at 48 kHz).
const TUNER_TAP_CAPACITY: usize = 4_096;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub last_preset: Option<String>,
    /// Last directories a NAM / IR was loaded from (browser starting points).
    #[serde(default)]
    pub nam_dir: Option<String>,
    #[serde(default)]
    pub ir_dir: Option<String>,
}

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
    /// MIDI input port override (name substring or index); `None` follows
    /// `midi.json` / first available port.
    pub midi_port: Option<String>,
}

/// A live MIDI input: the connection, its event stream, and the mapping.
struct MidiRuntime {
    _conn: lh_midi::MidiConnection,
    rx: std::sync::mpsc::Receiver<lh_midi::MidiEvent>,
    map: lh_midi::MidiMap,
}

/// A running pedalboard: audio streams live, handles on this side.
pub struct Session {
    pub chain: ChainHandle,
    pub nam: AssetHandle<NamAsset>,
    pub cab: AssetHandle<IrAsset>,
    pub nam_ref: Option<AssetRef>,
    pub ir_ref: Option<AssetRef>,
    pub sample_rate: u32,
    pub config: AppConfig,
    runner: DuplexRunner,
    tuner_tap: Option<rtrb::Consumer<f32>>,
    midi: Option<MidiRuntime>,
    /// Human-readable MIDI connection state for status lines.
    pub midi_status: String,
}

impl Session {
    /// Build the full pedalboard —
    /// gate → comp → drive → amp → eq → mod → delay → reverb → cab → limiter
    /// — and start the stream.
    pub fn start(opts: &SessionOpts) -> Result<Self, lh_io::IoError> {
        let (nam_amp, nam_handle) = NamAmp::new();
        let (cab, cab_handle) = CabIr::new();
        let effects: Vec<Box<dyn Effect>> = vec![
            Box::new(NoiseGate::new()),
            Box::new(Compressor::new()),
            Box::new(Drive::new()),
            Box::new(nam_amp),
            Box::new(Eq::new()),
            Box::new(Modulation::new()),
            Box::new(Delay::new()),
            Box::new(Reverb::new()),
            Box::new(cab),
            Box::new(Limiter::new()),
        ];
        let (mut chain, chain_handle) = build_chain(effects);

        let tuner_tap = if opts.tuner_tap {
            let (producer, consumer) = rtrb::RingBuffer::new(TUNER_TAP_CAPACITY);
            chain.set_input_tap(producer);
            Some(consumer)
        } else {
            None
        };

        let runner_opts = RunnerOpts {
            input: opts.input.clone(),
            output: opts.output.clone(),
            sample_rate: opts.sample_rate,
            buffer: opts.buffer,
            in_channel: opts.in_channel,
            gain_db: opts.gain_db,
            prefill_blocks: opts.prefill_blocks,
        };
        let runner = DuplexRunner::start(&runner_opts, move |info| {
            chain.prepare(info.sample_rate);
            Box::new(move |block: &mut [f32]| chain.process(block))
        })?;

        let (midi, midi_status) = connect_midi(opts.midi_port.as_deref());

        Ok(Self {
            chain: chain_handle,
            nam: nam_handle,
            cab: cab_handle,
            nam_ref: None,
            ir_ref: None,
            sample_rate: runner.sample_rate,
            config: load_config(),
            runner,
            tuner_tap,
            midi,
            midi_status,
        })
    }

    pub fn description(&self) -> &str {
        &self.runner.description
    }

    pub fn stats(&self) -> Snapshot {
        self.runner.stats()
    }

    /// The tuner's raw-input consumer; the GUI takes it once at startup.
    pub fn take_tuner_tap(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.tuner_tap.take()
    }

    /// The audio thread never deallocates: retired assets die here.
    /// Call periodically from the control loop / frame tick.
    pub fn collect_garbage(&mut self) {
        self.nam.collect_garbage();
        self.cab.collect_garbage();
    }

    /// Apply everything the foot controller sent since the last call.
    /// Returns user-facing lines describing what happened.
    pub fn drain_midi(&mut self) -> Vec<String> {
        let Some(midi) = &self.midi else {
            return Vec::new();
        };
        let events: Vec<lh_midi::MidiEvent> = midi.rx.try_iter().collect();
        let actions: Vec<lh_midi::Action> = events
            .iter()
            .filter_map(|e| midi.map.action_for(e))
            .collect();

        let mut lines = Vec::new();
        for action in actions {
            match action {
                lh_midi::Action::LoadPreset(name) => match self.load_preset(&name) {
                    Ok(mut msgs) => lines.append(&mut msgs),
                    Err(e) => lines.push(format!("midi: preset {name:?}: {e}")),
                },
                lh_midi::Action::LoadPresetIndex(index) => {
                    match list_presets().get(index as usize) {
                        Some(name) => {
                            let name = name.clone();
                            match self.load_preset(&name) {
                                Ok(mut msgs) => lines.append(&mut msgs),
                                Err(e) => lines.push(format!("midi: preset {name:?}: {e}")),
                            }
                        }
                        None => lines.push(format!("midi: no preset at PC {index}")),
                    }
                }
                lh_midi::Action::SetParam { slot, param, norm } => {
                    let real = self
                        .chain
                        .descriptors()
                        .iter()
                        .find(|d| d.key == slot)
                        .and_then(|d| d.params.iter().find(|p| p.key == param))
                        .map(|p| (p.range.to_real(norm), p.range.label(p.range.to_real(norm))));
                    match real {
                        Some((real, label)) => match self.chain.set_param(&slot, &param, real) {
                            Ok(applied) => lines.push(match label {
                                Some(label) => format!("midi: {slot}.{param} = {label}"),
                                None => format!(
                                    "midi: {slot}.{param} = {:.2} {}",
                                    applied.real, applied.unit
                                ),
                            }),
                            Err(e) => lines.push(format!("midi: {e}")),
                        },
                        None => lines.push(format!("midi: unknown target {slot}.{param}")),
                    }
                }
                lh_midi::Action::SetActive { slot, active } => {
                    match self.chain.set_active(&slot, active) {
                        Ok(()) => lines.push(format!(
                            "midi: {slot} {}",
                            if active { "on" } else { "off" }
                        )),
                        Err(e) => lines.push(format!("midi: {e}")),
                    }
                }
            }
        }
        lines
    }

    /// Preset to load on startup: explicit override, else the last one used.
    pub fn initial_preset(&self, requested: Option<String>) -> Option<String> {
        requested.or_else(|| self.config.last_preset.clone())
    }

    pub fn remember_preset(&mut self, name: &str) {
        self.config.last_preset = Some(name.to_string());
        save_config(&self.config);
    }

    /// Loaded asset file names for display, `"-"` when empty.
    pub fn asset_names(&self) -> (String, String) {
        let name = |r: &Option<AssetRef>| {
            r.as_ref()
                .and_then(|a| Path::new(&a.path).file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".into())
        };
        (name(&self.nam_ref), name(&self.ir_ref))
    }

    // --- assets ---

    pub fn load_nam(&mut self, path: &Path) -> Result<String, String> {
        let (asset, info) = load_nam_file(path, self.sample_rate).map_err(|e| e.to_string())?;
        let loudness = info
            .loudness_db
            .map(|l| format!("{l:.1} dB → normalized to -18 dB"))
            .unwrap_or_else(|| "unknown (no normalization)".into());
        if self.nam.install(asset).is_err() {
            return Err("install queue full, try again".into());
        }
        self.nam_ref = asset_ref_for(path);
        self.config.nam_dir = parent_dir(path);
        save_config(&self.config);
        Ok(format!(
            "nam: {} loaded ({} @ {} Hz, loudness {})",
            file_name(path),
            info.architecture,
            info.sample_rate,
            loudness,
        ))
    }

    pub fn load_ir(&mut self, path: &Path) -> Result<String, String> {
        let (asset, info) =
            lh_assets::load_ir(path, self.sample_rate).map_err(|e| e.to_string())?;
        if self.cab.install(asset).is_err() {
            return Err("install queue full, try again".into());
        }
        self.ir_ref = asset_ref_for(path);
        self.config.ir_dir = parent_dir(path);
        save_config(&self.config);
        let mut notes = Vec::new();
        if info.resampled {
            notes.push(format!(
                "resampled {} → {} Hz",
                info.source_rate, info.engine_rate
            ));
        }
        if info.trimmed {
            notes.push(format!("trimmed to {:.0} ms", info.seconds() * 1e3));
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(", "))
        };
        Ok(format!(
            "ir: {} loaded, {} samples = {:.0} ms{}",
            file_name(path),
            info.used_samples,
            info.seconds() * 1e3,
            notes,
        ))
    }

    /// Returns true when there was something to unload.
    pub fn unload_nam(&mut self) -> bool {
        let had = self.nam.clear();
        if had {
            self.nam_ref = None;
        }
        had
    }

    pub fn unload_ir(&mut self) -> bool {
        let had = self.cab.clear();
        if had {
            self.ir_ref = None;
        }
        had
    }

    // --- presets ---

    /// Save the current chain + assets. Returns the saved path message.
    pub fn save_preset(&mut self, name: &str) -> Result<String, String> {
        if !valid_preset_name(name) {
            return Err("preset names use letters, digits, - and _ only".into());
        }
        let dir = presets_dir().ok_or("cannot determine $HOME")?;
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: name.to_string(),
            chain: self.chain.snapshot_chain(),
            assets: PresetAssets {
                nam: self.nam_ref.clone(),
                ir: self.ir_ref.clone(),
            },
        };
        let path = dir.join(format!("{name}.json"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, preset.to_json_pretty()).map_err(|e| e.to_string())?;
        self.remember_preset(name);
        Ok(format!("saved {}", path.display()))
    }

    /// Load a preset by name: chain state, then both assets. Returns all
    /// user-facing lines (warnings included) in order.
    pub fn load_preset(&mut self, name: &str) -> Result<Vec<String>, String> {
        let dir = presets_dir().ok_or("cannot determine $HOME")?;
        let path = dir.join(format!("{name}.json"));
        let json = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let preset = Preset::from_json(&json).map_err(|e| e.to_string())?;

        let mut lines = Vec::new();
        let warnings = self
            .chain
            .apply_preset_chain(&preset.chain)
            .map_err(|e| e.to_string())?;
        lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));

        // Presets from older chains don't mention newer slots, which the
        // forward-compat rule appends at the end — after the limiter. The
        // limiter is the safety net and always runs last.
        let keys = self.chain.order_keys();
        if keys.contains(&"limiter") && keys.last() != Some(&"limiter") {
            let reordered: Vec<&str> = keys
                .iter()
                .copied()
                .filter(|k| *k != "limiter")
                .chain(std::iter::once("limiter"))
                .collect();
            self.chain
                .set_order(&reordered)
                .map_err(|e| e.to_string())?;
            lines.push("limiter moved back to the end of the chain".into());
        }

        self.apply_asset(preset.assets.nam.as_ref(), &dir, AssetKind::Nam, &mut lines);
        self.apply_asset(preset.assets.ir.as_ref(), &dir, AssetKind::Ir, &mut lines);

        lines.push(format!(
            "preset {name:?} loaded — chain: {}",
            self.chain.order_keys().join(" → ")
        ));
        self.remember_preset(name);
        Ok(lines)
    }

    fn apply_asset(
        &mut self,
        reference: Option<&AssetRef>,
        fallback_dir: &Path,
        kind: AssetKind,
        lines: &mut Vec<String>,
    ) {
        match reference {
            Some(r) => match lh_assets::resolve_asset(r, Some(fallback_dir)) {
                Ok((path, warnings)) => {
                    lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));
                    let loaded = match kind {
                        AssetKind::Nam => self.load_nam(&path),
                        AssetKind::Ir => self.load_ir(&path),
                    };
                    match loaded {
                        Ok(msg) => lines.push(msg),
                        Err(e) => lines.push(format!("error: {e}")),
                    }
                }
                Err(e) => lines.push(format!("error: {e}")),
            },
            None => {
                match kind {
                    AssetKind::Nam => self.unload_nam(),
                    AssetKind::Ir => self.unload_ir(),
                };
            }
        }
    }
}

enum AssetKind {
    Nam,
    Ir,
}

// --- disk layout & helpers ---

pub fn app_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".lion-heart"))
}

pub fn presets_dir() -> Option<PathBuf> {
    app_dir().map(|d| d.join("presets"))
}

/// Sorted preset names on disk (empty when none).
pub fn list_presets() -> Vec<String> {
    let Some(dir) = presets_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json"))
                .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

pub fn valid_preset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Read `~/.lion-heart/midi.json` (defaults when absent, warning on bad JSON).
fn load_midi_map() -> (lh_midi::MidiMap, Option<String>) {
    let Some(path) = app_dir().map(|d| d.join("midi.json")) else {
        return (lh_midi::MidiMap::default(), None);
    };
    match std::fs::read_to_string(&path) {
        Ok(json) => match lh_midi::MidiMap::from_json(&json) {
            Ok(map) => (map, None),
            Err(e) => (
                lh_midi::MidiMap::default(),
                Some(format!("{}: {e}", path.display())),
            ),
        },
        Err(_) => (lh_midi::MidiMap::default(), None),
    }
}

/// Try to bring up MIDI: never fatal — a pedalboard without a foot
/// controller must still start. Zero config connects the first port; PC `n`
/// then loads the n-th preset.
fn connect_midi(override_port: Option<&str>) -> (Option<MidiRuntime>, String) {
    let (map, warning) = load_midi_map();
    let selector = override_port
        .map(str::to_string)
        .or_else(|| map.input.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let result = lh_midi::connect(selector.as_deref(), tx);
    let with_warning = |status: String| match &warning {
        Some(w) => format!("{status} — warning: {w}"),
        None => status,
    };
    match result {
        Ok(conn) => {
            let status = with_warning(format!("midi: {}", conn.port_name));
            (
                Some(MidiRuntime {
                    _conn: conn,
                    rx,
                    map,
                }),
                status,
            )
        }
        Err(e) => (None, with_warning(format!("midi: none ({e})"))),
    }
}

fn load_config() -> AppConfig {
    app_dir()
        .map(|d| d.join("config.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(config: &AppConfig) {
    let Some(dir) = app_dir() else { return };
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

fn asset_ref_for(path: &Path) -> Option<AssetRef> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match lh_assets::hash_file(&canonical) {
        Ok(sha256) => Some(AssetRef {
            path: canonical.display().to_string(),
            sha256,
        }),
        Err(e) => {
            eprintln!("warning: could not hash asset: {e}");
            None
        }
    }
}

fn parent_dir(path: &Path) -> Option<String> {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .parent()
        .map(|p| p.display().to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
