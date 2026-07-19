//! The engine session shared by the jam REPL and the GUI: builds the
//! pedalboard chain, starts the duplex stream, loads assets, and persists
//! presets/config under `~/.lion-heart/`.
//!
//! Feedback discipline: operations return message strings instead of
//! printing, so the REPL can `println!` them and the GUI can show them in a
//! status line. `Err` is a single user-facing error message.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use lh_core::preset::{AssetRef, PRESET_SCHEMA_VERSION, Preset, PresetAssets, SNAPSHOT_SLOTS};
use lh_dsp::Effect;
use lh_dsp::blocks::swap::{AssetHandle, asset_channel};
use lh_dsp::cab::{CabIr, IrAsset};
use lh_dsp::drive::Drive;
use lh_dsp::dynamics::Compressor;
use lh_dsp::dynamics::Limiter;
use lh_dsp::dynamics::NoiseGate;
use lh_dsp::eq::Eq;
use lh_dsp::filter::Filter;
use lh_dsp::modulation::Modulation;
use lh_dsp::time::Delay;
use lh_dsp::time::Reverb;
use lh_engine::{ChainHandle, build_chain};

// The ~/.lion-heart disk layout lives in lh-assets, shared with the plugin
// (the preset list order is a cross-binary contract).
pub use lh_assets::{app_dir, list_presets, presets_dir, read_preset_order, save_preset_order};
use lh_io::passthrough::{DuplexRunner, RunnerOpts};
use lh_io::stats::Snapshot;
use lh_nam::{NamAmp, NamAsset, load_nam_file};
use serde::{Deserialize, Serialize};

/// Samples of raw input buffered for the tuner (~85 ms at 48 kHz).
const TUNER_TAP_CAPACITY: usize = 4_096;
/// Samples of output buffered for the spectrum analyzer (~170 ms at 48 kHz).
const SPECTRUM_TAP_CAPACITY: usize = 8_192;

#[derive(Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub last_preset: Option<String>,
    /// Last directories a NAM / IR was loaded from (browser starting points).
    #[serde(default)]
    pub nam_dir: Option<String>,
    #[serde(default)]
    pub ir_dir: Option<String>,
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
}

fn spillover_default() -> bool {
    true
}

fn tempo_default() -> f32 {
    lh_core::tempo::DEFAULT_BPM
}

impl Default for AppConfig {
    /// Matches the serde field defaults — notably `spillover: true`, so a
    /// fresh config (no file) and a file missing the field agree.
    fn default() -> Self {
        Self {
            last_preset: None,
            nam_dir: None,
            ir_dir: None,
            input: None,
            output: None,
            buffer: None,
            in_channel: None,
            morph_ms: 0,
            spillover: spillover_default(),
            tempo_bpm: tempo_default(),
        }
    }
}

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
/// ([`Session::carry_over`] → [`Session::resume`]).
pub struct CarryOver {
    chain: Vec<lh_core::preset::SlotState>,
    nam: Option<AssetRef>,
    ir: Option<AssetRef>,
    ir_b: Option<AssetRef>,
    snapshots: BTreeMap<String, lh_core::preset::Snapshot>,
    active_snapshot: Option<String>,
}

/// A live MIDI input: the connection, its event stream, and the mapping.
struct MidiRuntime {
    _conn: lh_midi::MidiConnection,
    rx: std::sync::mpsc::Receiver<lh_midi::MidiEvent>,
    map: lh_midi::MidiMap,
    /// Soft-takeover state per controller number (PRD 008).
    pickup: std::collections::HashMap<u8, PickupState>,
    /// Armed MIDI-learn target: the next on-channel CC binds to it.
    learn: Option<(String, String)>,
}

/// One controller's soft-takeover engagement.
#[derive(Default)]
struct PickupState {
    engaged: bool,
    /// The last shaped position, for crossing detection.
    last: Option<f32>,
}

/// How close (normalized) a pickup-gated pedal must land to the parameter
/// to engage without sweeping across it.
const PICKUP_WINDOW: f32 = 0.02;

impl PickupState {
    /// Feed one shaped pedal position given the parameter's current value;
    /// returns whether the controller is (now) engaged. Engagement happens
    /// on a sweep across the value or a landing within [`PICKUP_WINDOW`].
    fn feed(&mut self, current: f32, shaped: f32) -> bool {
        if self.engaged {
            return true;
        }
        let crossed = self
            .last
            .is_some_and(|prev| (prev - current) * (shaped - current) <= 0.0);
        let close = (shaped - current).abs() <= PICKUP_WINDOW;
        self.last = Some(shaped);
        self.engaged = crossed || close;
        self.engaged
    }
}

/// A running pedalboard: audio streams live, handles on this side.
pub struct Session {
    pub chain: ChainHandle,
    pub nam: AssetHandle<NamAsset>,
    pub cab: AssetHandle<IrAsset>,
    pub nam_ref: Option<AssetRef>,
    pub ir_ref: Option<AssetRef>,
    /// The cab's optional blend IR (a second mic/cabinet, ADR 015).
    pub ir_b_ref: Option<AssetRef>,
    pub sample_rate: u32,
    pub config: AppConfig,
    runner: DuplexRunner,
    tuner_tap: Option<rtrb::Consumer<f32>>,
    spectrum_tap: Option<rtrb::Consumer<f32>>,
    midi: Option<MidiRuntime>,
    /// Human-readable MIDI connection state for status lines.
    pub midi_status: String,
    /// Scenes for the loaded preset (PRD 009), keyed by letter; the active
    /// one; and an in-flight morph, ticked on the control loop.
    snapshots: BTreeMap<String, lh_core::preset::Snapshot>,
    active_snapshot: Option<String>,
    morph: Option<Morph>,
}

/// An in-progress snapshot morph (PRD 009): the value trajectory from the
/// pre-switch scene to the target, plus its wall-clock window. The
/// interpolation math is pure and unit-tested; the session feeds it a
/// progress fraction each control-loop tick and pushes the resulting norms.
struct Morph {
    steps: Vec<MorphStep>,
    started: Instant,
    dur_secs: f32,
}

struct MorphStep {
    handle: String,
    param: String,
    /// Normalized endpoints — log-ranged params morph musically in norm.
    from: f32,
    to: f32,
}

/// A param whose norm moves by less than this over a morph is dropped (a
/// switch only touches what actually differs).
const MORPH_EPS: f32 = 1e-4;

/// One snapshot chip's state for the GUI (PRD 009).
pub struct SnapshotChip {
    pub letter: &'static str,
    /// A scene is stored in this slot.
    pub populated: bool,
    /// The active scene.
    pub active: bool,
    /// Active and the live values have drifted from what is stored.
    pub dirty: bool,
}

/// Normalize a snapshot selector to a canonical letter, or an error.
fn normalize_snapshot_letter(letter: &str) -> Result<String, String> {
    let up = letter.trim().to_uppercase();
    if SNAPSHOT_SLOTS.contains(&up.as_str()) {
        Ok(up)
    } else {
        Err(format!(
            "snapshot must be one of {}",
            SNAPSHOT_SLOTS.join("/")
        ))
    }
}

/// Whether a stored scene matches the live one within value tolerance
/// (same active flags and real values on every slot the scene names).
fn scenes_match(stored: &lh_core::preset::Snapshot, live: &lh_core::preset::Snapshot) -> bool {
    stored.slots.iter().all(|(handle, s)| {
        live.slots.get(handle).is_some_and(|l| {
            s.active == l.active
                && s.values.iter().all(|(param, v)| {
                    l.values
                        .get(param)
                        .is_some_and(|lv| (lv - v).abs() <= v.abs().max(1.0) * 1e-3)
                })
        })
    })
}

impl Morph {
    /// Keep only the steps that actually move.
    fn build(started: Instant, dur_secs: f32, steps: Vec<MorphStep>) -> Self {
        let steps = steps
            .into_iter()
            .filter(|s| (s.to - s.from).abs() > MORPH_EPS)
            .collect();
        Self {
            steps,
            started,
            dur_secs,
        }
    }

    /// The (handle, param, norm) each step should hold at progress `t`.
    fn at(&self, t: f32) -> Vec<(&str, &str, f32)> {
        let t = t.clamp(0.0, 1.0);
        self.steps
            .iter()
            .map(|s| {
                (
                    s.handle.as_str(),
                    s.param.as_str(),
                    s.from + (s.to - s.from) * t,
                )
            })
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// The session asset a chain family mounts (hot-swapped off-thread). `IrB` is
/// not a family mount — it is the cab's optional blend IR (ADR 015), used by
/// the browser/unload routing only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Nam,
    Ir,
    IrB,
}

/// One buildable chain family: its descriptor (the key, display name, and
/// pedal faceplates all come from here), the asset it mounts — mounting
/// families stay chain singletons — and its constructor. `build` may rewire
/// the session's asset seams and flags which it replaced (amp, cab), so the
/// caller re-applies the loaded asset afterwards.
pub struct FamilyEntry {
    pub desc: &'static lh_core::FamilyDesc,
    pub asset: Option<AssetKind>,
    #[allow(clippy::type_complexity)]
    build: fn(
        &mut AssetHandle<NamAsset>,
        &mut AssetHandle<IrAsset>,
        &mut (bool, bool),
    ) -> Box<dyn Effect>,
}

/// Every chain family the session can build, in the default-chain (and add
/// menu) order — the one place that knows the full rig. Pinned to
/// [`lh_core::DEFAULT_CHAIN`] by a test, which also pins the plugin's fixed
/// chain to the same constant.
pub static FAMILY_REGISTRY: [FamilyEntry; 11] = [
    FamilyEntry {
        desc: &lh_dsp::dynamics::gate::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(NoiseGate::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::filter::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Filter::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::dynamics::comp::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Compressor::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::drive::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Drive::new()),
    },
    FamilyEntry {
        desc: &lh_nam::FAMILY,
        asset: Some(AssetKind::Nam),
        build: |nam, _, rebuilt| {
            let (amp, handle) = NamAmp::new();
            *nam = handle;
            rebuilt.0 = true;
            Box::new(amp)
        },
    },
    FamilyEntry {
        desc: &lh_dsp::eq::chain::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Eq::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::modulation::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Modulation::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::time::delay::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Delay::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::time::reverb::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Reverb::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::cab::FAMILY,
        asset: Some(AssetKind::Ir),
        build: |_, cab, rebuilt| {
            let (cab_fx, handle) = CabIr::new();
            *cab = handle;
            rebuilt.1 = true;
            Box::new(cab_fx)
        },
    },
    FamilyEntry {
        desc: &lh_dsp::dynamics::limiter::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(Limiter::new()),
    },
];

/// The registry entry for a family key, `None` when unknown.
pub fn family_entry(key: &str) -> Option<&'static FamilyEntry> {
    FAMILY_REGISTRY.iter().find(|e| e.desc.key == key)
}

/// The asset a family mounts, if any. Instance handles equal family keys
/// for the mounting families (they are singletons), so slot handles work.
pub fn asset_kind(family_key: &str) -> Option<AssetKind> {
    family_entry(family_key).and_then(|e| e.asset)
}

/// Build a fresh effect for a family key (PRD 002's factory seam — the
/// registry owns the concrete effect crates).
fn build_family_effect(
    nam: &mut AssetHandle<NamAsset>,
    cab: &mut AssetHandle<IrAsset>,
    rebuilt: &mut (bool, bool),
    key: &str,
) -> Option<Box<dyn Effect>> {
    family_entry(key).map(|entry| (entry.build)(nam, cab, rebuilt))
}

impl Session {
    /// Build the full pedalboard ([`lh_core::DEFAULT_CHAIN`], every registry
    /// family once, in order) and start the stream.
    pub fn start(opts: &SessionOpts) -> Result<Self, lh_io::IoError> {
        // Placeholder seams: building the default chain rewires both (it
        // contains amp and cab), so these never receive an install.
        let (_, mut nam_handle) = asset_channel::<NamAsset>();
        let (_, mut cab_handle) = asset_channel::<IrAsset>();
        let mut rebuilt = (false, false);
        let effects: Vec<Box<dyn Effect>> = lh_core::DEFAULT_CHAIN
            .iter()
            .map(|key| {
                build_family_effect(&mut nam_handle, &mut cab_handle, &mut rebuilt, key)
                    .expect("DEFAULT_CHAIN keys are registered (pinned by test)")
            })
            .collect();
        let (mut chain, mut chain_handle) = build_chain(effects);
        // Families with no transparent setting ship bypassed (PRD 007) —
        // the default rig must stay neutral until the player engages them.
        for key in lh_core::DEFAULT_CHAIN {
            if !lh_core::default_active(key) {
                let _ = chain_handle.set_active(key, false);
            }
        }

        let tuner_tap = if opts.tuner_tap {
            let (producer, consumer) = rtrb::RingBuffer::new(TUNER_TAP_CAPACITY);
            chain.set_input_tap(producer);
            Some(consumer)
        } else {
            None
        };
        let spectrum_tap = if opts.spectrum_tap {
            let (producer, consumer) = rtrb::RingBuffer::new(SPECTRUM_TAP_CAPACITY);
            chain.set_output_tap(producer);
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
            Box::new(move |left: &mut [f32], right: &mut [f32]| chain.process(left, right))
        })?;
        // Effects installed later are prepared control-side at this rate.
        chain_handle.set_sample_rate(runner.sample_rate);
        // Global output EQ (PRD 003): app-level, not part of presets.
        if let Err(e) = chain_handle.apply_eq_state(&load_global_eq()) {
            eprintln!("warning: global eq not applied: {e}");
        }

        let (midi, midi_status) = connect_midi(opts.midi_port.as_deref());

        Ok(Self {
            chain: chain_handle,
            nam: nam_handle,
            cab: cab_handle,
            nam_ref: None,
            ir_ref: None,
            ir_b_ref: None,
            sample_rate: runner.sample_rate,
            config: load_config(),
            runner,
            tuner_tap,
            spectrum_tap,
            midi,
            midi_status,
            snapshots: BTreeMap::new(),
            active_snapshot: None,
            morph: None,
        })
    }

    /// Snapshot everything that must survive a stream restart (device or
    /// buffer change): chain state and the loaded asset references.
    pub fn carry_over(&self) -> CarryOver {
        CarryOver {
            chain: self.chain.snapshot_chain(),
            nam: self.nam_ref.clone(),
            ir: self.ir_ref.clone(),
            ir_b: self.ir_b_ref.clone(),
            snapshots: self.snapshots.clone(),
            active_snapshot: self.active_snapshot.clone(),
        }
    }

    /// Start a fresh session with `opts` and restore a [`CarryOver`] onto it.
    /// The previous session must already be dropped — two sessions would race
    /// for the same devices. Returns the restore messages (warnings, asset
    /// loads) alongside the session.
    pub fn resume(
        opts: &SessionOpts,
        carry: &CarryOver,
    ) -> Result<(Self, Vec<String>), lh_io::IoError> {
        let mut session = Self::start(opts)?;
        let mut lines = Vec::new();
        match session.apply_chain_states(&carry.chain) {
            Ok(warnings) => lines.extend(warnings.into_iter().map(|w| format!("warning: {w}"))),
            Err(e) => lines.push(format!("warning: chain state not restored: {e}")),
        }
        // Assets reload from their canonical paths — a rate change re-runs
        // NAM validation and IR resampling against the new stream.
        let fallback = presets_dir().unwrap_or_default();
        session.apply_asset(carry.nam.as_ref(), &fallback, AssetKind::Nam, &mut lines);
        session.apply_cab(
            carry.ir.as_ref(),
            carry.ir_b.as_ref(),
            &fallback,
            &mut lines,
        );
        // Scenes ride across the restart (a device change must not wipe them).
        session.snapshots = carry.snapshots.clone();
        session.active_snapshot = carry.active_snapshot.clone();
        Ok((session, lines))
    }

    pub fn description(&self) -> &str {
        &self.runner.description
    }

    /// Resolved device names of the running stream (exact, for the settings
    /// panel's preselection).
    pub fn io_names(&self) -> (&str, &str) {
        (&self.runner.in_name, &self.runner.out_name)
    }

    pub fn stats(&self) -> Snapshot {
        self.runner.stats()
    }

    /// The tuner's raw-input consumer; the GUI takes it once at startup.
    pub fn take_tuner_tap(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.tuner_tap.take()
    }

    /// The spectrum analyzer's post-output consumer (GUI, once at startup).
    pub fn take_spectrum_tap(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.spectrum_tap.take()
    }

    // --- global output EQ (PRD 003) ---

    pub fn eq_state(&self) -> &lh_core::global_eq::GlobalEqState {
        self.chain.eq_state()
    }

    /// Live band update (no disk write — call [`Self::save_global_eq`] at
    /// commit points: drag release, toggles, resets).
    pub fn set_eq_band(
        &mut self,
        index: usize,
        band: lh_core::global_eq::Band,
    ) -> Result<(), String> {
        self.chain
            .set_eq_band(index, band)
            .map_err(|e| e.to_string())
    }

    pub fn set_eq_active(&mut self, enabled: bool) -> Result<(), String> {
        self.chain
            .set_eq_active(enabled)
            .map_err(|e| e.to_string())?;
        self.save_global_eq();
        Ok(())
    }

    /// Reset the global EQ to its transparent default layout.
    pub fn reset_global_eq(&mut self) -> Result<(), String> {
        self.chain
            .apply_eq_state(&lh_core::global_eq::GlobalEqState::default())
            .map_err(|e| e.to_string())?;
        self.save_global_eq();
        Ok(())
    }

    /// Persist the current EQ state to `~/.lion-heart/global_eq.json`.
    pub fn save_global_eq(&self) {
        let Some(path) = global_eq_path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, self.chain.eq_state().to_json_pretty())
        };
        if let Err(e) = write() {
            eprintln!("warning: could not save global eq: {e}");
        }
    }

    /// The audio thread never deallocates: retired assets and effects die
    /// here. Call periodically from the control loop / frame tick.
    pub fn collect_garbage(&mut self) {
        self.nam.collect_garbage();
        self.cab.collect_garbage();
        self.chain.collect_garbage();
    }

    /// Apply preset chain states **including structure** (PRD 002): the
    /// session provides the effect factory; a rebuilt amp/cab gets the
    /// session's loaded asset re-applied by the caller (`load_preset` and
    /// `resume` both re-apply assets right after).
    fn apply_chain_states(
        &mut self,
        states: &[lh_core::preset::SlotState],
    ) -> Result<Vec<String>, String> {
        let mut rebuilt = (false, false);
        let Session {
            chain,
            nam,
            cab,
            config,
            ..
        } = &mut *self;
        let spillover = config.spillover;
        chain
            .apply_preset_chain(states, spillover, &mut |key| {
                build_family_effect(nam, cab, &mut rebuilt, key)
            })
            .map_err(|e| e.to_string())
    }

    /// Add a `family_key` instance at `position` (`None` = end). Returns
    /// user-facing lines: the new handle plus any asset reloads.
    pub fn add_slot(
        &mut self,
        family_key: &str,
        position: Option<usize>,
    ) -> Result<Vec<String>, String> {
        let Some(entry) = family_entry(family_key) else {
            let known: Vec<&str> = FAMILY_REGISTRY.iter().map(|e| e.desc.key).collect();
            return Err(format!(
                "unknown family {family_key:?} — one of: {}",
                known.join(", ")
            ));
        };
        if entry.asset.is_some() && self.chain.contains_family(family_key) {
            return Err(format!(
                "only one {family_key} per chain (it mounts the loaded asset)"
            ));
        }
        let mut rebuilt = (false, false);
        let effect = {
            let Session { nam, cab, .. } = &mut *self;
            (entry.build)(nam, cab, &mut rebuilt)
        };
        let handle = self
            .chain
            .install_slot(effect, position.unwrap_or(usize::MAX))
            .map_err(|e| e.to_string())?;
        let mut lines = vec![format!(
            "added {handle} — chain: {}",
            self.chain.order_handles().join(" → ")
        )];
        // A fresh amp/cab mounts nothing yet: re-apply the session's assets.
        let fallback = presets_dir().unwrap_or_default();
        if rebuilt.0 {
            let nam_ref = self.nam_ref.clone();
            self.apply_asset(nam_ref.as_ref(), &fallback, AssetKind::Nam, &mut lines);
        }
        if rebuilt.1 {
            let ir_ref = self.ir_ref.clone();
            self.apply_asset(ir_ref.as_ref(), &fallback, AssetKind::Ir, &mut lines);
        }
        Ok(lines)
    }

    /// Remove a slot instance by handle.
    pub fn remove_slot(&mut self, handle: &str) -> Result<String, String> {
        // A tailed slot (delay/reverb) spills — its tail rings out in a
        // spill lane rather than being cut (PRD 010) — when spillover is on.
        let spill = self.config.spillover && self.chain.slot_has_tail(handle);
        let verb = if spill {
            self.chain.spill_slot(handle).map_err(|e| e.to_string())?;
            "spilled"
        } else {
            self.chain.remove_slot(handle).map_err(|e| e.to_string())?;
            "removed"
        };
        // A spilled slot desyncs pickup like any structure change (PRD 008).
        self.midi_desync_slot(handle);
        Ok(format!(
            "{verb} {handle} — chain: {}",
            self.chain.order_handles().join(" → ")
        ))
    }

    /// Whether spillover is on (PRD 010).
    pub fn spillover(&self) -> bool {
        self.config.spillover
    }

    /// Toggle spillover and persist it.
    pub fn set_spillover(&mut self, on: bool) -> String {
        self.config.spillover = on;
        save_config(&self.config);
        format!("spillover {}", if on { "on" } else { "off" })
    }

    /// Apply everything the foot controller sent since the last call.
    /// Returns user-facing lines describing what happened.
    pub fn drain_midi(&mut self) -> Vec<String> {
        let Some(midi) = &self.midi else {
            return Vec::new();
        };
        let events: Vec<lh_midi::MidiEvent> = midi.rx.try_iter().collect();
        let mut lines = Vec::new();
        for event in events {
            self.apply_midi_event(event, &mut lines);
        }
        lines
    }

    fn apply_midi_event(&mut self, event: lh_midi::MidiEvent, lines: &mut Vec<String>) {
        let controller = match event {
            lh_midi::MidiEvent::ControlChange { controller, .. } => Some(controller),
            _ => None,
        };
        // An armed learn eats the first on-channel CC (PRD 008).
        if let Some(midi) = self.midi.as_mut()
            && midi.learn.is_some()
            && midi.map.on_channel(&event)
            && let Some(controller) = controller
        {
            let (slot, param) = midi.learn.take().expect("checked above");
            let displaced = midi.map.bind_cc(controller, &slot, &param);
            midi.pickup.remove(&controller);
            let target = format!("{slot}.{param}");
            lines.push(match displaced.filter(|old| *old != target) {
                Some(old) => format!("midi: learned CC {controller} → {target} (was {old})"),
                None => format!("midi: learned CC {controller} → {target}"),
            });
            if let Some(warning) = save_midi_map(&midi.map) {
                lines.push(warning);
            }
            return;
        }
        let Some(action) = self.midi.as_ref().and_then(|m| m.map.action_for(&event)) else {
            return;
        };
        match action {
            lh_midi::Action::LoadPreset(name) => match self.load_preset(&name) {
                Ok(mut msgs) => lines.append(&mut msgs),
                Err(e) => lines.push(format!("midi: preset {name:?}: {e}")),
            },
            lh_midi::Action::LoadPresetIndex(index) => match list_presets().get(index as usize) {
                Some(name) => {
                    let name = name.clone();
                    match self.load_preset(&name) {
                        Ok(mut msgs) => lines.append(&mut msgs),
                        Err(e) => lines.push(format!("midi: preset {name:?}: {e}")),
                    }
                }
                None => lines.push(format!("midi: no preset at PC {index}")),
            },
            lh_midi::Action::SetParam {
                slot,
                param,
                norm,
                pickup,
            } => {
                // Virtual `snapshot.<anything>` target (PRD 009): the CC
                // position picks a scene A–D. Only switch on change, or a
                // held pedal would re-trigger the morph every frame.
                if slot == "snapshot" {
                    let n = SNAPSHOT_SLOTS.len();
                    let idx = ((norm * n as f32) as usize).min(n - 1);
                    let letter = SNAPSHOT_SLOTS[idx];
                    if self.active_snapshot.as_deref() != Some(letter) {
                        match self.switch_snapshot(letter) {
                            Ok(msg) => lines.push(format!("midi: {msg}")),
                            Err(e) => lines.push(format!("midi: {e}")),
                        }
                    }
                    return;
                }
                // `slot.pedal` (and the pre-v3 aliases) selects a pedal;
                // everything else lands on the active pedal's knobs.
                if lh_engine::is_pedal_selector(&param) {
                    match self.chain.select_pedal_norm(&slot, norm) {
                        Ok(pedal) => lines.push(format!("midi: {slot}.pedal = {pedal}")),
                        Err(e) => lines.push(format!("midi: {e}")),
                    }
                    return;
                }
                // Soft-takeover: a desynced pedal stays silent until it
                // sweeps across the value it is mapped to (PRD 008).
                if pickup
                    && let Some(controller) = controller
                    && !self.pickup_engaged(controller, &slot, &param, norm)
                {
                    return;
                }
                match self.chain.param_desc(&slot, &param) {
                    Some(p) => {
                        let real = p.range.to_real(norm);
                        match self.chain.set_param(&slot, &param, real) {
                            Ok(applied) => lines.push(match p.range.label(applied.real) {
                                Some(label) => format!("midi: {slot}.{param} = {label}"),
                                None => format!(
                                    "midi: {slot}.{param} = {:.2} {}",
                                    applied.real, applied.unit
                                ),
                            }),
                            Err(e) => lines.push(format!("midi: {e}")),
                        }
                    }
                    None => lines.push(format!("midi: unknown target {slot}.{param}")),
                }
            }
            lh_midi::Action::SetActive { slot, active } => {
                if slot == "snapshot" {
                    lines.push(
                        "midi: map a controller to \"snapshot.select\" (a value \
                         picks scene A–D), not bare \"snapshot\""
                            .into(),
                    );
                    return;
                }
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

    /// Soft-takeover gate: `true` once this controller has engaged — swept
    /// across the parameter's current value (or landed within
    /// [`PICKUP_WINDOW`] of it) since the last desync.
    fn pickup_engaged(&mut self, controller: u8, slot: &str, param: &str, shaped: f32) -> bool {
        let current = self.chain.param_norm(slot, param);
        let Some(midi) = self.midi.as_mut() else {
            return true;
        };
        // Unknown target: don't gate — the apply path owns the error line.
        let Some(current) = current else {
            return true;
        };
        midi.pickup
            .entry(controller)
            .or_default()
            .feed(current, shaped)
    }

    /// Forget every controller's soft-takeover engagement (a preset load
    /// re-seats all values under the hardware).
    pub fn midi_desync_all(&mut self) {
        if let Some(midi) = self.midi.as_mut() {
            midi.pickup.clear();
        }
    }

    /// Desync the controllers riding one param (a GUI knob moved it away
    /// from under the pedal).
    pub fn midi_desync_param(&mut self, slot: &str, param: &str) {
        let target = format!("{slot}.{param}");
        if let Some(midi) = self.midi.as_mut() {
            let map = &midi.map;
            midi.pickup.retain(|cc, _| {
                map.cc
                    .get(&cc.to_string())
                    .is_none_or(|m| m.target() != target)
            });
        }
    }

    /// Desync every controller riding a slot (its pedal switched — the
    /// incoming pedal's values re-seat from its shadow memory).
    pub fn midi_desync_slot(&mut self, slot: &str) {
        let prefix = format!("{slot}.");
        if let Some(midi) = self.midi.as_mut() {
            let map = &midi.map;
            midi.pickup.retain(|cc, _| {
                map.cc
                    .get(&cc.to_string())
                    .is_none_or(|m| !m.target().starts_with(&prefix))
            });
        }
    }

    /// Arm MIDI learn: the next on-channel CC binds to `slot.param` and is
    /// persisted to `midi.json` (PRD 008).
    pub fn arm_midi_learn(&mut self, slot: &str, param: &str) -> Result<String, String> {
        if self.chain.param_desc(slot, param).is_none() {
            return Err(format!("unknown target {slot}.{param}"));
        }
        let Some(midi) = self.midi.as_mut() else {
            return Err("no MIDI input connected".into());
        };
        midi.learn = Some((slot.to_string(), param.to_string()));
        Ok(format!("midi: learning {slot}.{param} — move a controller"))
    }

    /// The armed learn target, if any.
    pub fn midi_learn_target(&self) -> Option<(&str, &str)> {
        self.midi
            .as_ref()
            .and_then(|m| m.learn.as_ref())
            .map(|(s, p)| (s.as_str(), p.as_str()))
    }

    /// Disarm learn; `true` if something was armed.
    pub fn cancel_midi_learn(&mut self) -> bool {
        self.midi.as_mut().and_then(|m| m.learn.take()).is_some()
    }

    /// The controller bound to `slot.param`, if any (knob badges).
    pub fn cc_binding(&self, slot: &str, param: &str) -> Option<u8> {
        self.midi
            .as_ref()
            .and_then(|m| m.map.cc_for_param(slot, param))
    }

    /// Remove `slot.param`'s binding and persist the map.
    pub fn clear_cc_binding(&mut self, slot: &str, param: &str) -> Result<String, String> {
        let Some(midi) = self.midi.as_mut() else {
            return Err("no MIDI input connected".into());
        };
        match midi.map.unbind_param(slot, param) {
            Some(cc) => {
                midi.pickup.remove(&cc);
                let mut msg = format!("midi: cleared CC {cc} → {slot}.{param}");
                if let Some(warning) = save_midi_map(&midi.map) {
                    msg.push_str(&format!(" ({warning})"));
                }
                Ok(msg)
            }
            None => Err(format!("no CC bound to {slot}.{param}")),
        }
    }

    // --- snapshots / scenes (PRD 009) ---

    /// Store the current live scene (per-slot active + selected pedal's
    /// values) into slot `letter` (A–D). Becomes the active scene.
    pub fn store_snapshot(&mut self, letter: &str) -> Result<String, String> {
        let letter = normalize_snapshot_letter(letter)?;
        let scene = self.chain.capture_scene();
        self.snapshots.insert(letter.clone(), scene);
        self.active_snapshot = Some(letter.clone());
        self.morph = None;
        Ok(format!("snapshot {letter} stored"))
    }

    /// Switch to scene `letter`, morphing over the app's `morph_ms`.
    pub fn switch_snapshot(&mut self, letter: &str) -> Result<String, String> {
        let letter = normalize_snapshot_letter(letter)?;
        if !self.snapshots.contains_key(&letter) {
            return Err(format!("snapshot {letter} is empty — store it first"));
        }
        let secs = self.config.morph_ms as f32 / 1000.0;
        self.apply_snapshot(&letter, secs);
        Ok(if secs > 0.0 {
            format!("snapshot {letter} (morph {} ms)", self.config.morph_ms)
        } else {
            format!("snapshot {letter}")
        })
    }

    /// Apply scene `letter` over `morph_secs` (0 = instant). Flips bypass now
    /// (the engine crossfades it) and either sets every value immediately or
    /// starts a morph the control loop advances. A no-op if the letter is
    /// empty; handles/params the board no longer has are skipped.
    fn apply_snapshot(&mut self, letter: &str, morph_secs: f32) {
        let Some(target) = self.snapshots.get(letter).cloned() else {
            return;
        };
        let mut steps = Vec::new();
        for (handle, slot) in &target.slots {
            let _ = self.chain.set_active(handle, slot.active);
            for (param, real) in &slot.values {
                let Some(desc) = self.chain.param_desc(handle, param) else {
                    continue; // unknown handle/param: forward-compat skip
                };
                let to = desc.range.to_norm(desc.range.clamp(*real));
                let from = self.chain.param_norm(handle, param).unwrap_or(to);
                steps.push(MorphStep {
                    handle: handle.clone(),
                    param: param.clone(),
                    from,
                    to,
                });
            }
        }
        if morph_secs > 0.0 {
            let morph = Morph::build(Instant::now(), morph_secs, steps);
            // t=0 is the current state; let the loop advance from here.
            self.morph = (!morph.is_empty()).then_some(morph);
        } else {
            for step in &steps {
                if let Some(desc) = self.chain.param_desc(&step.handle, &step.param) {
                    let real = desc.range.to_real(step.to);
                    let _ = self.chain.set_param(&step.handle, &step.param, real);
                }
            }
            self.morph = None;
        }
        self.active_snapshot = Some(letter.to_string());
        // Scene values moved out from under the pedals: pickup re-engages.
        self.midi_desync_all();
    }

    /// Advance an in-flight morph to `now`; clears it when complete. Called
    /// on the control loop (GUI frame tick / REPL poll). Cheap and idle when
    /// no morph is running.
    pub fn tick_morph(&mut self, now: Instant) {
        let (updates, done) = {
            let Some(morph) = &self.morph else {
                return;
            };
            let t = if morph.dur_secs <= 0.0 {
                1.0
            } else {
                (now.duration_since(morph.started).as_secs_f32() / morph.dur_secs).clamp(0.0, 1.0)
            };
            let updates: Vec<(String, String, f32)> = morph
                .at(t)
                .into_iter()
                .map(|(h, p, n)| (h.to_string(), p.to_string(), n))
                .collect();
            (updates, t >= 1.0)
        };
        for (handle, param, norm) in updates {
            if let Some(desc) = self.chain.param_desc(&handle, &param) {
                let real = desc.range.to_real(norm);
                let _ = self.chain.set_param(&handle, &param, real);
            }
        }
        if done {
            self.morph = None;
        }
    }

    /// Whether a morph is currently animating (the GUI keeps redrawing knobs
    /// while it is).
    pub fn is_morphing(&self) -> bool {
        self.morph.is_some()
    }

    pub fn morph_ms(&self) -> u32 {
        self.config.morph_ms
    }

    /// Set the morph time (clamped 0–2000 ms) and persist it.
    pub fn set_morph_ms(&mut self, ms: u32) -> String {
        self.config.morph_ms = ms.min(2_000);
        save_config(&self.config);
        format!("morph time {} ms", self.config.morph_ms)
    }

    /// The rig's global tempo (ADR 014).
    pub fn tempo_bpm(&self) -> f32 {
        self.config.tempo_bpm
    }

    /// Set the global tempo (clamped to the musical range) and persist it.
    /// Application to the locked controls happens on the next
    /// [`Session::tick_tempo`] (the control loop runs it every tick; the REPL
    /// calls it explicitly).
    pub fn set_tempo_bpm(&mut self, bpm: f32) -> String {
        self.config.tempo_bpm = lh_core::tempo::clamp_bpm(bpm);
        save_config(&self.config);
        format!("tempo ♩ = {:.0} bpm", self.config.tempo_bpm)
    }

    /// Re-derive every tempo-locked control from the global BPM. Called on the
    /// control loop (GUI frame tick / REPL poll) after [`Session::tick_morph`].
    /// Delegates to [`lh_engine::ChainHandle::apply_tempo_sync`]; returns
    /// whether any control moved, so the GUI can refresh just the faceplate
    /// that changed.
    pub fn tick_tempo(&mut self) -> bool {
        self.chain.apply_tempo_sync(self.config.tempo_bpm)
    }

    /// Per-letter chip state for the GUI (PRD 009): populated, active, and
    /// (for the active one) whether the live scene has drifted from stored.
    pub fn snapshot_chips(&self) -> Vec<SnapshotChip> {
        let live = self.chain.capture_scene();
        SNAPSHOT_SLOTS
            .iter()
            .map(|&letter| {
                let stored = self.snapshots.get(letter);
                let active = self.active_snapshot.as_deref() == Some(letter);
                SnapshotChip {
                    letter,
                    populated: stored.is_some(),
                    active,
                    dirty: active && stored.is_some_and(|s| !scenes_match(s, &live)),
                }
            })
            .collect()
    }

    /// Preset to load on startup: explicit override, else the last one used.
    pub fn initial_preset(&self, requested: Option<String>) -> Option<String> {
        requested.or_else(|| self.config.last_preset.clone())
    }

    pub fn remember_preset(&mut self, name: &str) {
        self.config.last_preset = Some(name.to_string());
        save_config(&self.config);
    }

    /// Persist the applied I/O configuration (GUI settings panel). These
    /// become the defaults for the next launch; explicit CLI flags still win.
    pub fn remember_io(&mut self, opts: &SessionOpts) {
        self.config.input = opts.input.clone();
        self.config.output = opts.output.clone();
        self.config.buffer = Some(opts.buffer.unwrap_or(0));
        self.config.in_channel = Some(opts.in_channel);
        save_config(&self.config);
    }

    /// Loaded asset file names for display, `"-"` when empty.
    pub fn asset_names(&self) -> (String, String) {
        (asset_name(&self.nam_ref), asset_name(&self.ir_ref))
    }

    /// The cab's blend-IR file name, or `"-"` when none is loaded (ADR 015).
    pub fn ir_b_name(&self) -> String {
        asset_name(&self.ir_b_ref)
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

    /// (Re)decode the cab from its current primary + blend IR refs and install
    /// the combined asset in one hot-swap (ADR 015). Both files are re-read
    /// (control-thread, cheap) so whichever IRs are set ride the single swap;
    /// no primary IR clears the cab. Returns the primary IR's info for status.
    fn rebuild_cab(&mut self) -> Result<Option<lh_assets::IrInfo>, String> {
        let Some(a_ref) = self.ir_ref.clone() else {
            self.cab.clear();
            return Ok(None);
        };
        let (a, info) = lh_assets::load_ir_pair(Path::new(&a_ref.path), self.sample_rate)
            .map_err(|e| e.to_string())?;
        let b = match &self.ir_b_ref {
            Some(b_ref) => Some(
                lh_assets::load_ir_pair(Path::new(&b_ref.path), self.sample_rate)
                    .map_err(|e| e.to_string())?
                    .0,
            ),
            None => None,
        };
        if self.cab.install(Box::new(IrAsset { a, b })).is_err() {
            return Err("install queue full, try again".into());
        }
        Ok(Some(info))
    }

    /// Human-readable load note for an IR (resample/trim caveats).
    fn ir_note(path: &Path, info: &lh_assets::IrInfo) -> String {
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
        format!(
            "{}, {} samples = {:.0} ms{}",
            file_name(path),
            info.used_samples,
            info.seconds() * 1e3,
            notes,
        )
    }

    /// Load the cab's **primary** IR. Any loaded blend IR is preserved and
    /// re-installed alongside it.
    pub fn load_ir(&mut self, path: &Path) -> Result<String, String> {
        let prev = self.ir_ref.clone();
        self.ir_ref = asset_ref_for(path);
        match self.rebuild_cab() {
            Ok(Some(info)) => {
                self.config.ir_dir = parent_dir(path);
                save_config(&self.config);
                Ok(format!("ir: {} loaded", Self::ir_note(path, &info)))
            }
            Ok(None) => Ok("ir cleared".into()),
            Err(e) => {
                self.ir_ref = prev; // roll back on failure — keep the old cab
                Err(e)
            }
        }
    }

    /// Load the cab's **blend** IR (a second mic/cabinet, ADR 015). Requires a
    /// primary IR already loaded; the `blend` knob crossfades between them.
    pub fn load_ir_b(&mut self, path: &Path) -> Result<String, String> {
        if self.ir_ref.is_none() {
            return Err("load a primary cab IR first, then add a blend IR".into());
        }
        let prev = self.ir_b_ref.clone();
        self.ir_b_ref = asset_ref_for(path);
        match self.rebuild_cab() {
            Ok(_) => {
                self.config.ir_dir = parent_dir(path);
                save_config(&self.config);
                Ok(format!(
                    "ir blend: {} loaded — dial the cab `blend` knob",
                    file_name(path)
                ))
            }
            Err(e) => {
                self.ir_b_ref = prev; // roll back on failure
                Err(e)
            }
        }
    }

    /// Restore the cab from a preset / carry-over: set both IR refs (resolving
    /// each against `fallback_dir`) and install them together. No primary IR
    /// clears the cab; a blend IR without a primary is dropped.
    fn apply_cab(
        &mut self,
        ir: Option<&AssetRef>,
        ir_b: Option<&AssetRef>,
        fallback_dir: &Path,
        lines: &mut Vec<String>,
    ) {
        let Some(a_ref) = ir else {
            self.unload_ir(); // clears both refs + the cab
            return;
        };
        match lh_assets::resolve_asset(a_ref, Some(fallback_dir)) {
            Ok((a_path, warnings)) => {
                lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));
                // Set the blend ref first so the primary's load installs both
                // in one swap.
                self.ir_b_ref =
                    ir_b.and_then(|r| match lh_assets::resolve_asset(r, Some(fallback_dir)) {
                        Ok((p, w)) => {
                            lines.extend(w.into_iter().map(|w| format!("warning: {w}")));
                            asset_ref_for(&p)
                        }
                        Err(e) => {
                            lines.push(format!("error: blend ir: {e}"));
                            None
                        }
                    });
                match self.load_ir(&a_path) {
                    Ok(msg) => lines.push(msg),
                    Err(e) => lines.push(format!("error: {e}")),
                }
            }
            Err(e) => lines.push(format!("error: {e}")),
        }
    }

    /// Returns true when there was something to unload.
    pub fn unload_nam(&mut self) -> bool {
        let had = self.nam.clear();
        if had {
            self.nam_ref = None;
        }
        had
    }

    /// Unload the whole cab (both the primary and any blend IR).
    pub fn unload_ir(&mut self) -> bool {
        let had = self.cab.clear();
        self.ir_ref = None;
        self.ir_b_ref = None;
        had
    }

    /// Unload only the blend IR, leaving the primary cab in place.
    pub fn unload_ir_b(&mut self) -> bool {
        if self.ir_b_ref.is_none() {
            return false;
        }
        self.ir_b_ref = None;
        let _ = self.rebuild_cab(); // reinstall the primary alone
        true
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
                ir_b: self.ir_b_ref.clone(),
            },
            snapshots: self.snapshots.clone(),
            active_snapshot: self.active_snapshot.clone(),
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
        // The preset defines the chain structure (PRD 002): survivors keep
        // their state, missing instances are built, leftovers removed.
        let warnings = self.apply_chain_states(&preset.chain)?;
        lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));

        self.apply_asset(preset.assets.nam.as_ref(), &dir, AssetKind::Nam, &mut lines);
        self.apply_cab(
            preset.assets.ir.as_ref(),
            preset.assets.ir_b.as_ref(),
            &dir,
            &mut lines,
        );

        // Scenes come with the preset (PRD 009); apply the saved active one
        // instantly (no morph on load — it re-asserts values the baseline
        // chain already loaded).
        self.snapshots = preset.snapshots;
        self.active_snapshot = None;
        self.morph = None;
        if let Some(letter) = preset.active_snapshot {
            if self.snapshots.contains_key(&letter) {
                self.apply_snapshot(&letter, 0.0);
            }
            let count = self.snapshots.len();
            if count > 0 {
                lines.push(format!("scenes: {count} (active {letter})"));
            }
        }

        lines.push(format!(
            "preset {name:?} loaded — chain: {}",
            self.chain.order_handles().join(" → ")
        ));
        self.remember_preset(name);
        // Every param may have moved out from under a pedal: pickup-gated
        // controllers must re-engage before they speak again (PRD 008).
        self.midi_desync_all();
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
                        AssetKind::IrB => self.load_ir_b(&path),
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
                    AssetKind::IrB => self.unload_ir_b(),
                };
            }
        }
    }

    /// Delete a saved preset. Clears the remembered "last preset" if it
    /// pointed here, so a deleted name is not reloaded on the next launch, and
    /// prunes it from any custom order.
    pub fn delete_preset(&mut self, name: &str) -> Result<String, String> {
        let dir = presets_dir().ok_or("cannot determine $HOME")?;
        let path = delete_preset_file(&dir, name)?;
        if self.config.last_preset.as_deref() == Some(name) {
            self.config.last_preset = None;
            save_config(&self.config);
        }
        maintain_preset_order(|o| o.retain(|n| n != name));
        Ok(format!("deleted {}", path.display()))
    }

    /// Rename a preset on disk (its internal `name` field follows). Refuses
    /// to overwrite an existing target; keeps "last preset" and the custom
    /// order pointed at it (so it holds its position).
    pub fn rename_preset(&mut self, old: &str, new: &str) -> Result<String, String> {
        let dir = presets_dir().ok_or("cannot determine $HOME")?;
        copy_preset_file(&dir, old, new, true)?;
        if self.config.last_preset.as_deref() == Some(old) {
            self.remember_preset(new);
        }
        maintain_preset_order(|o| {
            for n in o.iter_mut() {
                if n == old {
                    *n = new.to_string();
                }
            }
        });
        Ok(format!("renamed {old:?} → {new:?}"))
    }

    /// Copy a preset to a new name (its internal `name` follows). Refuses to
    /// overwrite; leaves the active preset unchanged and, in a custom order,
    /// drops the copy right after its source.
    pub fn duplicate_preset(&mut self, src: &str, new: &str) -> Result<String, String> {
        let dir = presets_dir().ok_or("cannot determine $HOME")?;
        copy_preset_file(&dir, src, new, false)?;
        maintain_preset_order(|o| {
            if let Some(i) = o.iter().position(|n| n == src) {
                o.insert(i + 1, new.to_string());
            }
        });
        Ok(format!("copied {src:?} → {new:?}"))
    }
}

// --- disk layout & helpers ---

pub fn global_eq_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("global_eq.json"))
}

/// Read `~/.lion-heart/global_eq.json` (transparent default when absent,
/// warning on bad JSON).
fn load_global_eq() -> lh_core::global_eq::GlobalEqState {
    let Some(path) = global_eq_path() else {
        return lh_core::global_eq::GlobalEqState::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(json) => match lh_core::global_eq::GlobalEqState::from_json(&json) {
            Ok(state) => state,
            Err(e) => {
                eprintln!("warning: {}: {e} — using defaults", path.display());
                lh_core::global_eq::GlobalEqState::default()
            }
        },
        Err(_) => lh_core::global_eq::GlobalEqState::default(),
    }
}

pub fn valid_preset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Read and migrate a preset file into memory (shared by load + management).
fn read_preset_file(path: &Path) -> Result<Preset, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Preset::from_json(&json).map_err(|e| e.to_string())
}

/// Delete `{dir}/{name}.json`, returning its path. Errors if it is absent.
fn delete_preset_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = dir.join(format!("{name}.json"));
    if !path.is_file() {
        return Err(format!("no preset named {name:?}"));
    }
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Copy `{src}.json` → `{new}.json` under `dir`, rewriting the stored `name`
/// to `new`; `remove_src` turns the copy into a rename. Returns the new path.
/// Backs both [`Session::rename_preset`] and [`Session::duplicate_preset`].
fn copy_preset_file(dir: &Path, src: &str, new: &str, remove_src: bool) -> Result<PathBuf, String> {
    let from = dir.join(format!("{src}.json"));
    let to = dir.join(format!("{new}.json"));
    preset_copy_guard(src, new, from.is_file(), to.exists())?;
    let mut preset = read_preset_file(&from)?;
    preset.name = new.to_string();
    std::fs::write(&to, preset.to_json_pretty()).map_err(|e| e.to_string())?;
    if remove_src {
        std::fs::remove_file(&from).map_err(|e| e.to_string())?;
    }
    Ok(to)
}

/// Pure precondition check for a rename/duplicate: valid new name, distinct
/// names, source present, target free. Split out so it is unit-testable
/// without touching the disk.
fn preset_copy_guard(
    src: &str,
    new: &str,
    src_exists: bool,
    dst_exists: bool,
) -> Result<(), String> {
    if !valid_preset_name(new) {
        return Err("preset names use letters, digits, - and _ only".into());
    }
    if src == new {
        return Err("source and target names are the same".into());
    }
    if !src_exists {
        return Err(format!("no preset named {src:?}"));
    }
    if dst_exists {
        return Err(format!("a preset named {new:?} already exists"));
    }
    Ok(())
}

/// Keep `preset_order` coherent after a rename/delete/duplicate: apply `edit`
/// to the saved order and rewrite it. No-op when the user has no custom order
/// yet — everything simply stays alphabetical.
fn maintain_preset_order(edit: impl FnOnce(&mut Vec<String>)) {
    let mut order = read_preset_order();
    if order.is_empty() {
        return;
    }
    edit(&mut order);
    save_preset_order(&order);
}

/// A quick, human-facing digest of a preset file for the management page.
/// Even a broken file yields an `error`-tagged digest, so the page can still
/// list (and offer to delete) it.
#[derive(Debug, Clone)]
pub struct PresetInfo {
    pub name: String,
    /// "gate → drive → hall": each slot's pedal name (family key when it has
    /// none), bypassed slots parenthesized.
    pub chain: String,
    pub slots: usize,
    pub has_nam: bool,
    pub has_ir: bool,
    pub scenes: usize,
    /// Set when the file could not be read/parsed (schema too new, bad JSON).
    pub error: Option<String>,
}

/// Read `~/.lion-heart/presets/{name}.json` into a display digest.
pub fn preset_info(name: &str) -> PresetInfo {
    let mut info = PresetInfo {
        name: name.to_string(),
        chain: String::new(),
        slots: 0,
        has_nam: false,
        has_ir: false,
        scenes: 0,
        error: None,
    };
    let Some(dir) = presets_dir() else {
        info.error = Some("cannot determine $HOME".into());
        return info;
    };
    match read_preset_file(&dir.join(format!("{name}.json"))) {
        Ok(preset) => {
            info.chain = chain_summary(&preset);
            info.slots = preset.chain.len();
            info.has_nam = preset.assets.nam.is_some();
            info.has_ir = preset.assets.ir.is_some();
            info.scenes = preset.snapshots.len();
        }
        Err(e) => info.error = Some(e),
    }
    info
}

/// A compact "gate → drive → hall" chain string. Pure — takes the parsed
/// preset — so it is testable without the disk.
fn chain_summary(preset: &Preset) -> String {
    if preset.chain.is_empty() {
        return "passthrough".to_string();
    }
    preset
        .chain
        .iter()
        .map(|slot| {
            let name = slot.pedal.as_deref().unwrap_or(&slot.key);
            if slot.active {
                name.to_string()
            } else {
                format!("({name})")
            }
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Write `~/.lion-heart/midi.json` (learn/unbind persist the whole map,
/// keeping input/channel/pc_presets). A warning line on failure.
fn save_midi_map(map: &lh_midi::MidiMap) -> Option<String> {
    let Some(dir) = app_dir() else {
        return Some("warning: cannot determine $HOME — midi map not saved".into());
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("midi.json"), map.to_json_pretty())
    };
    write()
        .err()
        .map(|e| format!("warning: could not save midi map: {e}"))
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
                    pickup: std::collections::HashMap::new(),
                    learn: None,
                }),
                status,
            )
        }
        Err(e) => (None, with_warning(format!("midi: none ({e})"))),
    }
}

pub(crate) fn load_config() -> AppConfig {
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

/// An asset ref's file name for display, or `"-"` when unset.
fn asset_name(reference: &Option<AssetRef>) -> String {
    reference
        .as_ref()
        .and_then(|a| Path::new(&a.path).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".into())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_the_default_chain_and_its_invariants() {
        let keys: Vec<&str> = FAMILY_REGISTRY.iter().map(|e| e.desc.key).collect();
        assert_eq!(
            keys,
            lh_core::DEFAULT_CHAIN,
            "registry order is the default chain"
        );
        for (i, a) in keys.iter().enumerate() {
            // Trailing digits are reserved for instance handles ("drive2");
            // the engine's handle parser depends on it.
            assert!(
                !a.ends_with(|c: char| c.is_ascii_digit()),
                "family key {a:?} must not end in a digit"
            );
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "family keys are unique");
            }
        }
        // Only the asset-mounting families are singletons.
        let mounting: Vec<&str> = FAMILY_REGISTRY
            .iter()
            .filter(|e| e.asset.is_some())
            .map(|e| e.desc.key)
            .collect();
        assert_eq!(mounting, ["amp", "cab"]);
    }

    #[test]
    fn every_registry_entry_builds_its_own_family() {
        let (_, mut nam) = asset_channel::<NamAsset>();
        let (_, mut cab) = asset_channel::<IrAsset>();
        let mut rebuilt = (false, false);
        for entry in &FAMILY_REGISTRY {
            let effect = build_family_effect(&mut nam, &mut cab, &mut rebuilt, entry.desc.key)
                .expect("registered family builds");
            assert!(
                std::ptr::eq(effect.family(), entry.desc),
                "{}: built effect must report the registry's own family",
                entry.desc.key
            );
        }
        assert!(rebuilt.0 && rebuilt.1, "amp and cab rewire their seams");
        assert!(build_family_effect(&mut nam, &mut cab, &mut rebuilt, "wah").is_none());
    }

    /// Soft-takeover (PRD 008): a desynced pedal is silent until it sweeps
    /// across the parameter's value (or lands next to it), then sticks.
    #[test]
    fn pickup_engages_on_crossing_or_landing() {
        // Param sits at 0.5; the pedal wakes up down at 0.1 and sweeps up.
        let mut state = PickupState::default();
        assert!(!state.feed(0.5, 0.1), "far below: stay silent");
        assert!(!state.feed(0.5, 0.3), "approaching: still silent");
        assert!(state.feed(0.5, 0.7), "swept across: engage");
        assert!(state.feed(0.5, 0.2), "engaged: every move applies");

        // Landing inside the window engages without a crossing.
        let mut state = PickupState::default();
        assert!(state.feed(0.5, 0.49), "close enough: engage immediately");

        // Crossing works downward too, and against a moving target.
        let mut state = PickupState::default();
        assert!(!state.feed(0.5, 0.9));
        assert!(state.feed(0.5, 0.4), "downward sweep engages");

        // The very first touch exactly on the value engages.
        let mut state = PickupState::default();
        assert!(state.feed(0.25, 0.25));
    }

    fn morph_step(handle: &str, from: f32, to: f32) -> MorphStep {
        MorphStep {
            handle: handle.into(),
            param: "x".into(),
            from,
            to,
        }
    }

    /// Morph (PRD 009): unchanged params drop out; the rest interpolate
    /// monotonically from the current value (t=0) to the target (t=1).
    #[test]
    fn morph_drops_noops_and_interpolates_endpoints() {
        let now = Instant::now();
        let m = Morph::build(
            now,
            1.0,
            vec![
                morph_step("drive", 0.2, 0.8),  // moves
                morph_step("comp", 0.5, 0.5),   // no-op, dropped
                morph_step("reverb", 0.9, 0.1), // moves down
            ],
        );
        assert_eq!(m.steps.len(), 2, "the no-op step is dropped");

        // t=0 is the starting values, t=1 the targets.
        let at0 = m.at(0.0);
        assert!((at0[0].2 - 0.2).abs() < 1e-6 && (at0[1].2 - 0.9).abs() < 1e-6);
        let at1 = m.at(1.0);
        assert!((at1[0].2 - 0.8).abs() < 1e-6 && (at1[1].2 - 0.1).abs() < 1e-6);

        // The midpoint sits strictly between, and motion is monotone.
        let mid = m.at(0.5);
        assert!(
            (mid[0].2 - 0.5).abs() < 1e-6,
            "up step halfway: {}",
            mid[0].2
        );
        assert!(
            (mid[1].2 - 0.5).abs() < 1e-6,
            "down step halfway: {}",
            mid[1].2
        );
        let (mut prev_up, mut prev_dn) = (at0[0].2, at0[1].2);
        for i in 1..=10 {
            let v = m.at(i as f32 / 10.0);
            assert!(v[0].2 >= prev_up - 1e-6, "up must not backtrack");
            assert!(v[1].2 <= prev_dn + 1e-6, "down must not backtrack");
            prev_up = v[0].2;
            prev_dn = v[1].2;
        }

        // t clamps: past the end stays at the target.
        assert!((m.at(1.5)[0].2 - 0.8).abs() < 1e-6);
    }

    #[test]
    fn snapshot_letters_are_validated() {
        assert_eq!(normalize_snapshot_letter("a").unwrap(), "A");
        assert_eq!(normalize_snapshot_letter(" c ").unwrap(), "C");
        assert!(normalize_snapshot_letter("E").is_err());
        assert!(normalize_snapshot_letter("").is_err());
    }

    #[test]
    fn scenes_match_within_tolerance() {
        use lh_core::preset::{Snapshot, SnapshotSlot};
        let scene = |gain: f32, active: bool| Snapshot {
            slots: BTreeMap::from([(
                "drive".to_string(),
                SnapshotSlot {
                    active,
                    values: BTreeMap::from([("gain".to_string(), gain)]),
                },
            )]),
        };
        assert!(scenes_match(&scene(5.0, true), &scene(5.0, true)));
        assert!(
            scenes_match(&scene(5.0, true), &scene(5.0005, true)),
            "tiny drift ok"
        );
        assert!(
            !scenes_match(&scene(5.0, true), &scene(6.0, true)),
            "value drift"
        );
        assert!(
            !scenes_match(&scene(5.0, true), &scene(5.0, false)),
            "bypass drift"
        );
    }

    // --- preset management (delete / rename / duplicate / digest) ---
    //
    // These exercise the disk helpers against an explicit temp dir, so they
    // never touch $HOME or config.json and stay parallel-safe.

    use lh_core::preset::SlotState;

    fn preset_tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lion-heart-preset-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_test_preset(dir: &Path, name: &str) {
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: name.to_string(),
            chain: vec![SlotState {
                key: "gate".into(),
                ..Default::default()
            }],
            assets: PresetAssets::default(),
            snapshots: BTreeMap::new(),
            active_snapshot: None,
        };
        std::fs::write(dir.join(format!("{name}.json")), preset.to_json_pretty()).unwrap();
    }

    #[test]
    fn copy_guard_rejects_bad_inputs() {
        assert!(
            preset_copy_guard("a", "a", true, false).is_err(),
            "same name"
        );
        assert!(
            preset_copy_guard("a", "bad name", true, false).is_err(),
            "invalid new name"
        );
        assert!(
            preset_copy_guard("a", "b", false, false).is_err(),
            "missing source"
        );
        assert!(
            preset_copy_guard("a", "b", true, true).is_err(),
            "target exists"
        );
        assert!(preset_copy_guard("a", "b", true, false).is_ok());
    }

    #[test]
    fn duplicate_keeps_source_and_rewrites_internal_name() {
        let dir = preset_tmp_dir("dup");
        write_test_preset(&dir, "lead");
        let to = copy_preset_file(&dir, "lead", "lead-copy", false).unwrap();
        assert!(dir.join("lead.json").is_file(), "source kept");
        assert_eq!(
            read_preset_file(&to).unwrap().name,
            "lead-copy",
            "internal name follows the file name"
        );
    }

    #[test]
    fn rename_moves_file_and_refuses_to_clobber() {
        let dir = preset_tmp_dir("rename");
        write_test_preset(&dir, "old");
        copy_preset_file(&dir, "old", "new", true).unwrap();
        assert!(!dir.join("old.json").exists(), "source removed");
        assert_eq!(read_preset_file(&dir.join("new.json")).unwrap().name, "new");

        write_test_preset(&dir, "keep");
        assert!(
            copy_preset_file(&dir, "new", "keep", true).is_err(),
            "won't overwrite"
        );
        assert!(dir.join("new.json").is_file(), "refused rename left source");
    }

    #[test]
    fn delete_removes_file_then_reports_missing() {
        let dir = preset_tmp_dir("del");
        write_test_preset(&dir, "gone");
        assert!(delete_preset_file(&dir, "gone").is_ok());
        assert!(!dir.join("gone.json").exists());
        assert!(
            delete_preset_file(&dir, "gone").is_err(),
            "second delete errors"
        );
    }

    #[test]
    fn chain_summary_reads_pedals_and_marks_bypass() {
        let mut preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: "x".into(),
            chain: vec![
                SlotState {
                    key: "gate".into(),
                    active: true,
                    ..Default::default()
                },
                SlotState {
                    key: "drive".into(),
                    active: false,
                    pedal: Some("evva".into()),
                    ..Default::default()
                },
            ],
            assets: PresetAssets::default(),
            snapshots: BTreeMap::new(),
            active_snapshot: None,
        };
        assert_eq!(chain_summary(&preset), "gate → (evva)");
        preset.chain.clear();
        assert_eq!(chain_summary(&preset), "passthrough");
    }
}
