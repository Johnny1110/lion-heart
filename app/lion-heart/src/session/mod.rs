//! The engine session shared by the jam REPL and the GUI: builds the
//! pedalboard chain, starts the duplex stream, loads assets, and persists
//! presets/config under `~/.lion-heart/`.
//!
//! Feedback discipline: operations return message strings instead of
//! printing, so the REPL can `println!` them and the GUI can show them in a
//! status line. `Err` is a single user-facing error message.

// Submodules (extracted from the original monolith):
pub(crate) mod asset;
pub(crate) mod config;
pub(crate) mod family;
pub(crate) mod global_eq;
pub(crate) mod looper;
pub(crate) mod midi;
pub(crate) mod practice;
pub(crate) mod preset;
pub(crate) mod record;
pub(crate) mod setlist_ops;
pub(crate) mod slot;
pub(crate) mod snapshot;
pub(crate) mod tempo;

// Re-exports that callers need:
pub use config::{AppConfig, CarryOver, SessionOpts, load_config};
pub use family::{AssetKind, FAMILY_REGISTRY, asset_kind, family_entry};
pub use looper::LooperLed;
pub use preset::{PresetInfo, preset_info};

// Re-export from lh_assets for callers:
pub use lh_assets::wav::WavBits;
pub use lh_assets::{list_presets, presets_dir, save_preset_order};

use snapshot::Morph;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Instant;

use lh_core::preset::{AssetRef, PRESET_SCHEMA_VERSION, PresetAssets, SNAPSHOT_SLOTS};
use lh_dsp::Effect;
use lh_dsp::blocks::swap::{AssetHandle, asset_channel};
use lh_dsp::cab::IrAsset;
use lh_engine::{ChainHandle, RecordTapState, build_chain};

use crate::leveling::Levels;
use crate::recorder::{RecStatus, RecSummary, Recorder};
use crate::setlist::{self, Setlists};
use lh_io::passthrough::{DuplexRunner, RunnerOpts};
use lh_io::stats::Snapshot;
use lh_nam::{NamAsset, load_nam_file};

use asset::{file_name, parent_dir};
use config::{recordings_dir, save_config};
pub(crate) use family::build_family_effect;
use global_eq::load_global_eq;
use midi::{MidiRuntime, connect_midi};
use practice::{
    AUX_RING_FRAMES, GrooveShared, GrooveSnapshot, MetroSnapshot, MetronomeShared, SongShared,
    spawn_player,
};
use tempo::TempoState;

/// Samples of raw input buffered for the tuner (~85 ms at 48 kHz).
const TUNER_TAP_CAPACITY: usize = 4_096;
/// Samples of output buffered for the spectrum analyzer (~170 ms at 48 kHz).
const SPECTRUM_TAP_CAPACITY: usize = 8_192;
/// Rate the recording tap rings are sized for (PRD 014). Fixed high value so
/// the buffer holds ≥1 s at any real stream rate — depth is only disk-stall
/// slack, so oversizing is harmless.
const RECORD_RING_RATE: u32 = 96_000;

/// A running pedalboard: audio streams live, handles on this side.
pub struct Session {
    pub chain: ChainHandle,
    pub nam: AssetHandle<NamAsset>,
    pub cab: AssetHandle<lh_dsp::cab::IrAsset>,
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
    /// Global tempo (PRD 012) — session-transient, never persisted.
    tempo: TempoState,
    /// Looper transport LED mirror (PRD 013), keyed by slot handle —
    /// session-transient (a restart re-prepares empty loops).
    looper_leds: std::collections::HashMap<String, LooperLed>,
    /// Metronome control shared with the aux player thread (PRD 019). The BPM
    /// tracks the global tempo; other settings are session-transient (carried
    /// across a device restart, not persisted).
    metronome: Arc<MetronomeShared>,
    /// Drum-groove control shared with the same player thread (PRD 019 Phase 2).
    groove: Arc<GrooveShared>,
    /// Song-player control shared with the player thread (PRD 019 Phase 3).
    song: Arc<SongShared>,
    /// Hands a decoded buffer to the player thread.
    song_tx: std::sync::mpsc::Sender<Arc<lh_dsp::practice::SongBuffer>>,
    /// Loader threads report completion here; the session drains it on tick.
    song_load_tx: std::sync::mpsc::Sender<SongLoad>,
    song_load_rx: std::sync::mpsc::Receiver<SongLoad>,
    /// The loaded buffer, kept so it can be re-sent to a fresh player thread
    /// after a device restart; and its GUI-side metadata.
    song_current: Option<Arc<lh_dsp::practice::SongBuffer>>,
    song_name: Option<String>,
    song_peaks: Vec<f32>,
    /// The aux player thread's handle, joined on drop.
    player_join: Option<JoinHandle<()>>,
    /// Monitor recorder (PRD 014): DI + wet tracks to disk. Owns the tap
    /// consumers; a take does not survive a device restart (a fresh session
    /// gets a fresh recorder, and the old one finalizes its WAV on drop).
    recorder: Recorder,
    /// Named setlists (PRD 016) — app-global, disk-backed. When one is active
    /// it drives prev/next and MIDI Program Change through its order.
    setlists: Setlists,
    /// Per-preset loudness trims (PRD 016) — app-global, disk-backed; applied
    /// to the output-stage master trim on preset load.
    levels: Levels,
}

impl Drop for Session {
    /// Stop the aux player thread cleanly (PRD 019): clear its run flag and join
    /// it before the stream tears down, so its aux producer is released while
    /// the chain (holding the consumer) is still alive on the audio thread.
    fn drop(&mut self) {
        self.metronome.running.store(false, Ordering::Relaxed);
        if let Some(join) = self.player_join.take() {
            let _ = join.join();
        }
    }
}

/// A completed background song decode (loader thread → session, PRD 019 Phase 3).
enum SongLoad {
    Loaded {
        song: Arc<lh_dsp::practice::SongBuffer>,
        name: String,
        peaks: Vec<f32>,
    },
    Failed(String),
}

// --- impl Session ---
//
// NOTE: This is the largest impl block in the crate (~1900 lines).
// Methods are grouped by concern but all live here for now.
// Future passes will move groups into submodules (e.g., session/preset.rs
// will gain `impl Session { fn save_preset(...) }` blocks).
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
        // Aux monitor lane (PRD 019): the player thread pushes the metronome
        // (and later drums/backing) into the chain, summed after the amp/limiter.
        let (aux_prod, aux_cons) = rtrb::RingBuffer::new(AUX_RING_FRAMES * 2);
        chain.set_aux_input(aux_cons);

        // Recording taps (PRD 014): DI at chain entry, wet after the output
        // stage. Always wired but dormant (one atomic load/block) until a take
        // arms them. Sized generously (~2 s @ 96k) and decoupled from the
        // stream rate — the depth only buys slack against a disk stall.
        let rec_state = Arc::new(RecordTapState::default());
        let rec_cap = Recorder::ring_capacity(RECORD_RING_RATE);
        let (di_prod, di_cons) = rtrb::RingBuffer::new(rec_cap);
        let (wet_prod, wet_cons) = rtrb::RingBuffer::new(rec_cap);
        chain.set_record_taps(
            di_prod,
            Arc::clone(&rec_state),
            wet_prod,
            Arc::clone(&rec_state),
        );

        // Config is needed before the stream for the metronome's initial tempo.
        let config = load_config();

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

        // Start the aux player thread now that the stream rate is known; the
        // click + groove follow the persisted global tempo, the song has its
        // own transport.
        let metronome = Arc::new(MetronomeShared::new(config.tempo_bpm));
        let groove = Arc::new(GrooveShared::new());
        let song = Arc::new(SongShared::new());
        let (song_tx, song_rx) = std::sync::mpsc::channel();
        let (song_load_tx, song_load_rx) = std::sync::mpsc::channel();
        let player_join = spawn_player(
            aux_prod,
            Arc::clone(&metronome),
            Arc::clone(&groove),
            Arc::clone(&song),
            song_rx,
            runner.sample_rate,
        );

        // Recorder owns the tap consumers; the WAV headers use the true stream
        // rate. Recording is app-global environment (like the metronome), so it
        // does not carry across a device restart.
        let recorder = Recorder::new(
            di_cons,
            wet_cons,
            rec_state,
            runner.sample_rate,
            recordings_dir(&config),
            WavBits::from_number(config.record_bits),
        );

        Ok(Self {
            chain: chain_handle,
            nam: nam_handle,
            cab: cab_handle,
            nam_ref: None,
            ir_ref: None,
            ir_b_ref: None,
            sample_rate: runner.sample_rate,
            config,
            runner,
            tuner_tap,
            spectrum_tap,
            midi,
            midi_status,
            snapshots: BTreeMap::new(),
            active_snapshot: None,
            morph: None,
            tempo: TempoState::default(),
            looper_leds: std::collections::HashMap::new(),
            metronome,
            groove,
            song,
            song_tx,
            song_load_tx,
            song_load_rx,
            song_current: None,
            song_name: None,
            song_peaks: Vec::new(),
            player_join: Some(player_join),
            recorder,
            // App-global, disk-backed (PRD 016): reloaded on every start, and
            // every mutation saves immediately, so a device restart preserves
            // them without threading through CarryOver.
            setlists: Setlists::load(),
            levels: Levels::load(),
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
            metronome: MetroSnapshot {
                enabled: self.metronome.enabled(),
                volume: self.metronome.volume(),
                beats_per_bar: self.metronome.beats_per_bar(),
                accent: self.metronome.accent(),
            },
            groove: GrooveSnapshot {
                enabled: self.groove.enabled(),
                pattern: self.groove.pattern(),
                volume: self.groove.volume(),
            },
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
        // The metronome rides across too (PRD 019) — the fresh player thread
        // already tracks the persisted tempo; restore the rest of its state.
        session.metronome.set_volume(carry.metronome.volume);
        session
            .metronome
            .set_beats_per_bar(carry.metronome.beats_per_bar);
        session.metronome.set_accent(carry.metronome.accent);
        if carry.metronome.enabled {
            session.metronome.set_enabled(true);
            session.metronome.request_restart();
        }
        session.groove.set_volume(carry.groove.volume);
        session.groove.set_pattern(carry.groove.pattern);
        if carry.groove.enabled {
            session.groove.set_enabled(true);
            session.groove.request_restart();
        }
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

    // --- global tempo: source (taps + MIDI clock) ---
    //
    // One rig BPM lives in `config.tempo_bpm` (persisted; see `tempo_bpm` /
    // `set_tempo_bpm` further below). This section is the *source* side —
    // turning tap gestures and MIDI-clock ticks into that BPM. Applying it to
    // the sync-locked delays/tremolo is `apply_tempo_now` / `tick_tempo`,
    // which delegate to `ChainHandle::apply_tempo_sync` (the note-division
    // math in `lh_core::tempo`, ADR 014).

    /// Preset to load on startup: explicit override, else the last one used.
    pub fn initial_preset(&self, requested: Option<String>) -> Option<String> {
        requested.or_else(|| self.config.last_preset.clone())
    }

    pub fn remember_preset(&mut self, name: &str) {
        self.config.last_preset = Some(name.to_string());
        save_config(&self.config);
    }

    /// The name of the most recently loaded preset, if any.
    pub fn current_preset(&self) -> Option<&str> {
        self.config.last_preset.as_deref()
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

    /// The current output-stage master loudness trim (dB).
    pub fn master_trim_db(&self) -> f32 {
        self.chain.master_trim_db()
    }

    /// The stored loudness target + per-preset trims (GUI/REPL read).
    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    /// Set (or clear, with `None`) the current preset's loudness trim, persist
    /// it to `levels.json`, and apply it live. Clamped to the trim range.
    pub fn set_preset_trim(&mut self, name: &str, trim_db: Option<f32>) -> Result<String, String> {
        let applied = match trim_db {
            Some(db) => {
                let db = db.clamp(-crate::leveling::MAX_TRIM_DB, crate::leveling::MAX_TRIM_DB);
                self.levels.trims.insert(name.to_string(), db);
                db
            }
            None => {
                self.levels.trims.remove(name);
                0.0
            }
        };
        self.levels.save();
        self.chain
            .set_master_trim_db(applied)
            .map_err(|e| e.to_string())?;
        Ok(format!("{name:?} trim {applied:+.1} dB"))
    }
}
#[cfg(test)]
mod tests {
    use super::midi::PickupState;
    use super::*;

    #[test]
    fn registry_covers_the_default_chain_and_its_invariants() {
        let keys: Vec<&str> = FAMILY_REGISTRY.iter().map(|e| e.desc.key).collect();
        // The default board is an in-order subsequence of the registry: every
        // shipped family is registered, in the same relative order, but the
        // registry may carry extra opt-in families that ship *off* the board —
        // `pitch` (ADR 016), the standalone-only `looper` (PRD 013), and the
        // `acoustic` simulator.
        let mut cursor = keys.iter();
        for want in lh_core::DEFAULT_CHAIN {
            assert!(
                cursor.any(|k| *k == want),
                "default-chain family {want:?} missing from the registry (or out of order)"
            );
        }
        // Everything the registry carries beyond the default board is off-board
        // by construction, so none of it may also appear in DEFAULT_CHAIN.
        let off_board: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| !lh_core::DEFAULT_CHAIN.contains(k))
            .collect();
        assert_eq!(
            off_board,
            ["pitch", "looper", "acoustic"],
            "off-board opt-in families, in registry order"
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
}
