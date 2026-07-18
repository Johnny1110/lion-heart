//! The Lion-Heart GUI (M4 "the face"): run the pedalboard without a terminal.
//!
//! Layout: a header with the view tabs (board · tuner · eq · live) and the
//! settings chip, a persistent preset bar (prev/next, picker, save — presets
//! are a first-class control, not a buried overlay), the chain strip (always
//! visible; clicking a card jumps back to the board view with that pedal's
//! panel open), the active view, and the status footer.
//!
//! Threading contract (CLAUDE.md rules): this UI thread owns the [`Session`]
//! — `ChainHandle` for lock-free param/bypass/order messages, asset handles
//! for hot-swaps — and polls `Telemetry` atomics plus the tuner tap on every
//! window frame. Nothing here ever blocks or allocates on the audio thread;
//! retired assets are collected on the frame tick.

mod browser;
mod eq;
mod knob;
mod meter;
mod spectrum;
mod theme;
mod tuner;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Canvas};
use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, space, text, text_input,
};
use iced::{Element, Length, Size, Subscription, Theme, window};
use lh_dsp::tuner::Tuner;
use lh_io::devices::DeviceDesc;

use crate::cli::GuiArgs;
pub use crate::session::AssetKind;
use crate::session::{FAMILY_REGISTRY, Session, SessionOpts, list_presets, load_config};
use browser::Browser;
use eq::EqPanel;
use lh_core::global_eq::{Band, BandKind};
use meter::{Ballistics, Meters};
use spectrum::SpectrumAnalyzer;
use tuner::{Reading, TunerDisplay};

/// Frames between retired-asset sweeps (~200 ms at 60 fps).
const GC_FRAMES: u32 = 12;
/// Frames between tuner estimates (~15 Hz at 60 fps).
const TUNER_FRAMES: u32 = 4;
/// Frames between spectrum updates (~30 Hz at 60 fps).
const SPECTRUM_FRAMES: u32 = 2;
/// Frames between preset-directory rescans (~1 Hz at 60 fps).
const PRESET_SCAN_FRAMES: u32 = 60;
/// Keep showing the last tuner reading this long after the note decays.
const TUNER_HOLD: Duration = Duration::from_millis(800);
const MAX_KNOBS: usize = 8;

pub fn run(args: GuiArgs) -> anyhow::Result<()> {
    iced::application(move || App::new(&args), App::update, App::view)
        .subscription(App::subscription)
        .theme(App::theme)
        .antialiasing(true)
        .window_size(Size::new(1120.0, 700.0))
        .title("Lion-Heart")
        .run()?;
    Ok(())
}

#[derive(Debug, Clone)]
pub enum Message {
    Frame(Instant),
    ToggleSlot(String),
    MoveSlotLeft(String),
    MoveSlotRight(String),
    /// Chain-strip drag editing: press starts a potential drag, entering
    /// another card marks the drop target (leaving clears it), release
    /// commits (same card = plain selection click).
    CardPress(usize),
    CardEnter(usize),
    CardExit(usize),
    CardRelease(usize),
    /// Insert a new family instance at the end of the chain.
    AddSlot(&'static str),
    /// Remove a slot instance by handle.
    RemoveSlot(String),
    /// Switch a slot's pedal (by key or display name).
    SelectPedal {
        slot: String,
        pedal: String,
    },
    Knob {
        slot: String,
        param: String,
        norm: f32,
    },
    OpenBrowser(AssetKind),
    BrowserNav(PathBuf),
    BrowserPick(PathBuf),
    BrowserClose,
    UnloadAsset(AssetKind),
    /// Switch the main panel to a view tab (board / tuner / eq / live).
    ShowPanel(Panel),
    /// Live global-EQ band edit; `commit` also persists to disk.
    EqBand {
        index: usize,
        band: Band,
        commit: bool,
    },
    /// Persist the EQ state (drag release).
    EqCommit,
    EqSelect(usize),
    EqKind(BandKind),
    EqMaster,
    EqFlat,
    ToggleSettings,
    SettingsInput(DeviceChoice),
    SettingsOutput(DeviceChoice),
    SettingsChannel(u16),
    SettingsBuffer(BufferChoice),
    /// Restart the audio session with the draft settings (handled at the
    /// [`App`] level: the running state is consumed and rebuilt).
    SettingsApply,
    PresetNameChanged(String),
    SavePreset,
    LoadPreset(String),
}

/// The view tabs in the header, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// The pedalboard: the selected slot's faceplate.
    Board,
    Tuner,
    /// Global output EQ editor with the live spectrum (PRD 003).
    Eq,
    /// Stage mode: big preset name, prev/next, big meters.
    Live,
}

/// What the main panel is showing. [`Panel`] tabs cover the first four;
/// settings and the asset browser open from their own controls.
enum View {
    Board,
    Tuner,
    Eq,
    Live,
    Browser(Browser),
    /// Audio I/O settings: devices, input channel, buffer size.
    Settings(SettingsDraft),
}

/// A device selection in the settings panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceChoice {
    Default,
    Named(String),
}

impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceChoice::Default => f.write_str("system default"),
            DeviceChoice::Named(name) => f.write_str(name),
        }
    }
}

/// A buffer-size selection in the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferChoice {
    Auto,
    Frames(u32),
}

impl std::fmt::Display for BufferChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BufferChoice::Auto => f.write_str("auto (device default)"),
            BufferChoice::Frames(n) => write!(f, "{n} frames"),
        }
    }
}

/// Pending audio settings: a device-list snapshot plus the user's choices.
/// Nothing takes effect until apply restarts the stream.
struct SettingsDraft {
    devices: Vec<DeviceDesc>,
    input: DeviceChoice,
    output: DeviceChoice,
    in_channel: u16,
    buffer: BufferChoice,
}

impl SettingsDraft {
    /// Open the panel mirroring the running configuration. `current_in` /
    /// `current_out` are the resolved device names, so an explicit CLI
    /// substring preselects the device it actually matched.
    fn open(opts: &SessionOpts, current_in: &str, current_out: &str) -> Self {
        Self::with_devices(
            lh_io::devices::enumerate().unwrap_or_default(),
            opts,
            current_in,
            current_out,
        )
    }

    fn with_devices(
        devices: Vec<DeviceDesc>,
        opts: &SessionOpts,
        current_in: &str,
        current_out: &str,
    ) -> Self {
        let choice = |spec: &Option<String>, name: &str| match spec {
            None => DeviceChoice::Default,
            Some(_) => DeviceChoice::Named(name.to_string()),
        };
        Self {
            devices,
            input: choice(&opts.input, current_in),
            output: choice(&opts.output, current_out),
            in_channel: opts.in_channel,
            buffer: match opts.buffer {
                None => BufferChoice::Auto,
                Some(n) => BufferChoice::Frames(n),
            },
        }
    }

    fn input_options(&self) -> Vec<DeviceChoice> {
        self.options(|d| d.input.is_some())
    }

    fn output_options(&self) -> Vec<DeviceChoice> {
        self.options(|d| d.output.is_some())
    }

    fn options(&self, has_port: impl Fn(&DeviceDesc) -> bool) -> Vec<DeviceChoice> {
        std::iter::once(DeviceChoice::Default)
            .chain(
                self.devices
                    .iter()
                    .filter(|d| has_port(d))
                    .map(|d| DeviceChoice::Named(d.name.clone())),
            )
            .collect()
    }

    /// The input-device description the draft currently points at.
    fn input_desc(&self) -> Option<&DeviceDesc> {
        match &self.input {
            DeviceChoice::Default => self.devices.iter().find(|d| d.is_default_input),
            DeviceChoice::Named(name) => self.devices.iter().find(|d| &d.name == name),
        }
    }

    fn output_desc(&self) -> Option<&DeviceDesc> {
        match &self.output {
            DeviceChoice::Default => self.devices.iter().find(|d| d.is_default_output),
            DeviceChoice::Named(name) => self.devices.iter().find(|d| &d.name == name),
        }
    }

    fn input_channels(&self) -> u16 {
        self.input_desc()
            .and_then(|d| d.input.as_ref())
            .map(|p| p.channels.max(1))
            .unwrap_or(1)
    }

    fn channel_options(&self) -> Vec<u16> {
        (1..=self.input_channels()).collect()
    }

    /// Standard block sizes, plus the current value when it is nonstandard
    /// (e.g. a `--buffer 48` launch).
    fn buffer_options(&self) -> Vec<BufferChoice> {
        let mut sizes = vec![32, 64, 128, 256, 512, 1024];
        if let BufferChoice::Frames(n) = self.buffer
            && !sizes.contains(&n)
        {
            sizes.push(n);
            sizes.sort_unstable();
        }
        std::iter::once(BufferChoice::Auto)
            .chain(sizes.into_iter().map(BufferChoice::Frames))
            .collect()
    }

    /// The draft as session options; everything not on the panel is kept.
    fn to_opts(&self, base: &SessionOpts) -> SessionOpts {
        let name = |choice: &DeviceChoice| match choice {
            DeviceChoice::Default => None,
            DeviceChoice::Named(name) => Some(name.clone()),
        };
        SessionOpts {
            input: name(&self.input),
            output: name(&self.output),
            in_channel: self.in_channel,
            buffer: match self.buffer {
                BufferChoice::Auto => None,
                BufferChoice::Frames(n) => Some(n),
            },
            ..base.clone()
        }
    }
}

/// One chain slot as the UI sees it, rebuilt from the engine snapshot.
struct SlotUi {
    /// Instance handle ("drive", "drive2", …) — the engine address.
    key: String,
    name: &'static str,
    active: bool,
    /// The active pedal's identity color (chain card, faceplate, knobs).
    color: iced::Color,
    /// The family's selectable pedals (len 1 for single-pedal slots).
    pedals: &'static [&'static lh_core::EffectDesc],
    active_pedal: usize,
    /// Params of the active pedal only — each pedal wears its own face.
    params: Vec<ParamUi>,
}

/// A chain-strip drag in progress.
struct DragState {
    from: usize,
    over: Option<usize>,
}

struct ParamUi {
    key: &'static str,
    name: &'static str,
    norm: f32,
    /// The faceplate default, normalized — knob double-click returns here.
    default_norm: f32,
    display: String,
    /// `Some((labels, current index))` for stepped params — rendered as a
    /// dropdown instead of a knob (drive model, modulation type).
    stepped: Option<(&'static [&'static str], usize)>,
}

enum App {
    Running(Box<Running>),
    /// Audio startup failed: show the error and the known fixes.
    Failed(String),
}

struct Running {
    session: Session,
    /// The options the running session was started with; the settings panel
    /// derives its draft from (and applies back onto) these.
    opts: SessionOpts,
    tap: Option<rtrb::Consumer<f32>>,
    tuner: Tuner,
    reading: Option<(Reading, Instant)>,
    spectrum_tap: Option<rtrb::Consumer<f32>>,
    analyzer: SpectrumAnalyzer,
    eq_selected: usize,
    eq_cache: canvas::Cache,
    slots: Vec<SlotUi>,
    selected: String,
    drag: Option<DragState>,
    view: View,
    preset_name: String,
    presets: Vec<String>,
    active_preset: Option<String>,
    status: String,
    ballistics: Ballistics,
    frame_count: u32,
    last_frame: Option<Instant>,
    frame_secs: f32,
    knob_caches: Vec<canvas::Cache>,
    meter_cache: canvas::Cache,
    live_meter_cache: canvas::Cache,
    tuner_cache: canvas::Cache,
}

impl App {
    fn new(args: &GuiArgs) -> Self {
        // Audio I/O saved from the settings panel fills in whatever the CLI
        // left unspecified; explicit flags always win.
        let saved = load_config();
        let opts = SessionOpts {
            input: args.io.input.clone().or_else(|| saved.input.clone()),
            output: args.io.output.clone().or_else(|| saved.output.clone()),
            sample_rate: args.io.sample_rate,
            buffer: match args.io.buffer.or(saved.buffer) {
                None => Some(64),
                Some(0) => None,
                Some(n) => Some(n),
            },
            in_channel: args.io.in_channel.or(saved.in_channel).unwrap_or(1),
            gain_db: args.gain_db,
            prefill_blocks: args.prefill_blocks,
            tuner_tap: true,
            spectrum_tap: true,
            midi_port: args.midi.clone(),
        };
        let mut session = match Session::start(&opts) {
            Ok(session) => session,
            Err(e) => return App::Failed(e.to_string()),
        };

        let mut status = format!("{} · {}", session.description(), session.midi_status);
        if let Some(name) = session.initial_preset(args.preset.clone()) {
            match session.load_preset(&name) {
                Ok(lines) => {
                    for line in &lines {
                        eprintln!("{line}");
                    }
                    status = lines.last().cloned().unwrap_or(status);
                }
                Err(e) => status = format!("preset {name:?}: {e}"),
            }
        }
        let active_preset = session.config.last_preset.clone();

        let tap = session.take_tuner_tap();
        let tuner = Tuner::new(session.sample_rate);
        let spectrum_tap = session.take_spectrum_tap();
        let analyzer = SpectrumAnalyzer::new(session.sample_rate);
        let mut running = Running {
            session,
            opts,
            tap,
            tuner,
            reading: None,
            spectrum_tap,
            analyzer,
            eq_selected: 2,
            eq_cache: canvas::Cache::new(),
            slots: Vec::new(),
            selected: String::new(),
            drag: None,
            view: View::Board,
            preset_name: active_preset.clone().unwrap_or_default(),
            presets: list_presets(),
            active_preset,
            status,
            ballistics: Ballistics::new(),
            frame_count: 0,
            last_frame: None,
            frame_secs: 1.0 / 60.0,
            knob_caches: (0..MAX_KNOBS).map(|_| canvas::Cache::new()).collect(),
            meter_cache: canvas::Cache::new(),
            live_meter_cache: canvas::Cache::new(),
            tuner_cache: canvas::Cache::new(),
        };
        running.refresh_slots();
        App::Running(Box::new(running))
    }

    fn update(&mut self, message: Message) {
        match message {
            // Applying settings consumes the running state (the session is
            // torn down and rebuilt), so it is handled at the App level.
            Message::SettingsApply => {
                if matches!(self, App::Running(_)) {
                    let App::Running(running) =
                        std::mem::replace(self, App::Failed("restarting audio…".into()))
                    else {
                        unreachable!("matched App::Running above");
                    };
                    *self = running.apply_settings();
                }
            }
            message => {
                if let App::Running(running) = self {
                    running.update(message);
                }
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        match self {
            App::Running(running) => running.view(),
            App::Failed(error) => container(
                column![
                    text("audio startup failed")
                        .size(20)
                        .color(theme::METER_HOT),
                    text(error.clone())
                        .size(14)
                        .color(theme::TEXT_BRIGHT)
                        .font(iced::Font::MONOSPACE),
                    text("fix the device setup and start Lion-Heart again")
                        .size(13)
                        .color(theme::TEXT_DIM),
                ]
                .spacing(16)
                .max_width(720),
            )
            .center(Length::Fill)
            .style(theme::root)
            .into(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        match self {
            App::Running(_) => window::frames().map(Message::Frame),
            App::Failed(_) => Subscription::none(),
        }
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

impl Running {
    /// Rebuild the UI mirror of the chain from the engine-side snapshot.
    fn refresh_slots(&mut self) {
        // families / handles / snapshot are all in chain order — one entry
        // per instance (duplicates included, PRD 002).
        let families = self.session.chain.families();
        let handles = self.session.chain.order_handles();
        self.slots = self
            .session
            .chain
            .snapshot_chain()
            .iter()
            .zip(families)
            .zip(handles)
            .map(|((slot, family), handle)| {
                // Knobs come straight from the active pedal's own faceplate
                // (PRD 001) — no shared tables, no caption remapping.
                let active_pedal = slot
                    .pedal
                    .as_deref()
                    .and_then(|key| family.pedal_index(key))
                    .unwrap_or(0);
                let desc = family.pedals[active_pedal];
                let values = slot.pedals.get(desc.key);
                SlotUi {
                    key: handle,
                    name: family.name,
                    active: slot.active,
                    color: theme::pedal_color(family.key, desc.key),
                    pedals: family.pedals,
                    active_pedal,
                    params: desc
                        .params
                        .iter()
                        .map(|p| {
                            let real = values
                                .and_then(|m| m.get(p.key))
                                .copied()
                                .unwrap_or(p.default);
                            ParamUi {
                                key: p.key,
                                name: p.name,
                                norm: p.range.to_norm(real),
                                default_norm: p.default_norm(),
                                display: p
                                    .range
                                    .label(real)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| format_value(real, p.unit)),
                                stepped: match p.range {
                                    lh_core::Range::Stepped { labels } => {
                                        Some((labels, p.range.clamp(real) as usize))
                                    }
                                    _ => None,
                                },
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        if !self.slots.iter().any(|s| s.key == self.selected)
            && let Some(first) = self.slots.first()
        {
            self.selected = first.key.clone();
        }
        for cache in &self.knob_caches {
            cache.clear();
        }
    }

    /// Refresh one param's UI mirror after a live knob move — the cheap
    /// path for drags; `refresh_slots` re-snapshots the entire chain.
    fn update_param_ui(&mut self, slot_key: &str, param_key: &str, real: f32) {
        let Some(desc) = self.session.chain.param_desc(slot_key, param_key) else {
            return;
        };
        let Some(slot) = self.slots.iter_mut().find(|s| s.key == slot_key) else {
            return;
        };
        let mut knob_index = 0;
        for param in &mut slot.params {
            let is_knob = param.stepped.is_none();
            if param.key == param_key {
                param.norm = desc.range.to_norm(real);
                param.display = desc
                    .range
                    .label(real)
                    .map(str::to_string)
                    .unwrap_or_else(|| format_value(real, desc.unit));
                if is_knob && let Some(cache) = self.knob_caches.get(knob_index) {
                    cache.clear();
                }
                return;
            }
            if is_knob {
                knob_index += 1;
            }
        }
    }

    /// Select the slot at a chain position and show its faceplate: whatever
    /// view is up, clicking the chain always lands on the board (the fix for
    /// "chain clicks do nothing while tuner/eq/live/settings is open").
    fn select_position(&mut self, position: usize) {
        if let Some(slot) = self.slots.get(position) {
            self.selected = slot.key.clone();
            self.view = View::Board;
            for cache in &self.knob_caches {
                cache.clear();
            }
        }
    }

    /// The preset `step` away from the active one; with none active, next
    /// starts at the first and prev at the last.
    fn preset_neighbor(&self, step: isize) -> Option<String> {
        if self.presets.is_empty() {
            return None;
        }
        let Some(current) = self.active_preset.as_deref() else {
            let index = if step > 0 { 0 } else { self.presets.len() - 1 };
            return Some(self.presets[index].clone());
        };
        let pos = self.presets.iter().position(|p| p == current)? as isize;
        let next = pos + step;
        (next >= 0 && (next as usize) < self.presets.len())
            .then(|| self.presets[next as usize].clone())
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Frame(now) => self.on_frame(now),
            Message::CardPress(position) => {
                self.drag = Some(DragState {
                    from: position,
                    over: None,
                });
            }
            Message::CardEnter(position) => {
                if let Some(drag) = &mut self.drag {
                    drag.over = (position != drag.from).then_some(position);
                }
            }
            Message::CardExit(position) => {
                if let Some(drag) = &mut self.drag
                    && drag.over == Some(position)
                {
                    drag.over = None;
                }
            }
            Message::CardRelease(position) => {
                let Some(drag) = self.drag.take() else {
                    self.select_position(position);
                    return;
                };
                if drag.from == position {
                    self.select_position(position);
                    return;
                }
                match self.session.chain.move_position(drag.from, position) {
                    Ok(()) => {
                        self.refresh_slots();
                        self.select_position(position);
                        self.status =
                            format!("chain: {}", self.session.chain.order_handles().join(" → "));
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
            Message::AddSlot(family) => match self.session.add_slot(family, None) {
                Ok(lines) => {
                    for line in &lines {
                        eprintln!("{line}");
                    }
                    self.status = lines.first().cloned().unwrap_or_default();
                    self.refresh_slots();
                    self.select_position(self.slots.len().saturating_sub(1));
                }
                Err(e) => self.status = e.to_string(),
            },
            Message::RemoveSlot(handle) => match self.session.remove_slot(&handle) {
                Ok(msg) => {
                    self.status = msg;
                    self.refresh_slots();
                }
                Err(e) => self.status = e.to_string(),
            },
            Message::ToggleSlot(key) => {
                let active = self
                    .slots
                    .iter()
                    .find(|s| s.key == key)
                    .map(|s| s.active)
                    .unwrap_or(true);
                match self.session.chain.set_active(&key, !active) {
                    Ok(()) => self.refresh_slots(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Message::MoveSlotLeft(key) => self.move_slot(&key, -1),
            Message::MoveSlotRight(key) => self.move_slot(&key, 1),
            Message::SelectPedal { slot, pedal } => {
                match self.session.chain.select_pedal(&slot, &pedal) {
                    Ok(name) => {
                        self.status = format!("{slot}: {name}");
                        self.refresh_slots();
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
            Message::Knob { slot, param, norm } => {
                let Some(desc) = self.session.chain.param_desc(&slot, &param) else {
                    return;
                };
                let stepped = matches!(desc.range, lh_core::Range::Stepped { .. });
                let real = desc.range.to_real(norm);
                match self.session.chain.set_param(&slot, &param, real) {
                    // A drag streams one message per mouse move: update the
                    // one dragged param in place instead of re-snapshotting
                    // the whole chain (and redrawing every knob) each event.
                    Ok(applied) if !stepped => self.update_param_ui(&slot, &param, applied.real),
                    Ok(_) => self.refresh_slots(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Message::OpenBrowser(kind) => {
                let remembered = match kind {
                    AssetKind::Nam => self.session.config.nam_dir.clone(),
                    AssetKind::Ir => self.session.config.ir_dir.clone(),
                };
                let start = remembered
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("/"));
                self.view = View::Browser(Browser::open(kind, start));
            }
            Message::BrowserNav(path) => {
                if let View::Browser(browser) = &mut self.view {
                    browser.navigate(path);
                }
            }
            Message::BrowserPick(path) => {
                if let View::Browser(browser) = &self.view {
                    let result = match browser.kind {
                        AssetKind::Nam => self.session.load_nam(&path),
                        AssetKind::Ir => self.session.load_ir(&path),
                    };
                    match result {
                        Ok(msg) => {
                            self.status = msg;
                            self.view = View::Board;
                        }
                        Err(e) => self.status = format!("error: {e}"),
                    }
                }
            }
            Message::BrowserClose => self.view = View::Board,
            Message::UnloadAsset(kind) => {
                let (had, label) = match kind {
                    AssetKind::Nam => (self.session.unload_nam(), "nam"),
                    AssetKind::Ir => (self.session.unload_ir(), "ir"),
                };
                if had {
                    self.status = format!("{label}: unloaded");
                }
            }
            Message::ShowPanel(panel) => self.show_panel(panel),
            Message::EqBand {
                index,
                band,
                commit,
            } => {
                match self.session.set_eq_band(index, band) {
                    Ok(()) => {
                        self.eq_selected = index;
                        let b = self.session.eq_state().bands[index];
                        self.status = format!(
                            "eq band {}: {} {:.0} Hz {:+.1} dB Q {:.2}{}",
                            index + 1,
                            b.kind,
                            b.freq,
                            b.gain_db,
                            b.q,
                            if b.enabled { "" } else { " (off)" },
                        );
                        if commit {
                            self.session.save_global_eq();
                        }
                    }
                    Err(e) => self.status = e,
                }
                self.eq_cache.clear();
            }
            Message::EqCommit => self.session.save_global_eq(),
            Message::EqSelect(index) => {
                self.eq_selected = index;
                self.eq_cache.clear();
            }
            Message::EqKind(kind) => {
                let mut band = self.session.eq_state().bands[self.eq_selected];
                band.kind = kind;
                if !kind.has_gain() {
                    band.gain_db = 0.0;
                }
                match self.session.set_eq_band(self.eq_selected, band) {
                    Ok(()) => self.session.save_global_eq(),
                    Err(e) => self.status = e,
                }
                self.eq_cache.clear();
            }
            Message::EqMaster => {
                let enabled = !self.session.eq_state().enabled;
                if let Err(e) = self.session.set_eq_active(enabled) {
                    self.status = e;
                } else {
                    self.status =
                        format!("global eq {}", if enabled { "enabled" } else { "bypassed" });
                }
                self.eq_cache.clear();
            }
            Message::EqFlat => {
                match self.session.reset_global_eq() {
                    Ok(()) => self.status = "global eq reset to flat".into(),
                    Err(e) => self.status = e,
                }
                self.eq_cache.clear();
            }
            Message::ToggleSettings => {
                self.view = match self.view {
                    View::Settings(_) => View::Board,
                    _ => {
                        let (in_name, out_name) = self.session.io_names();
                        View::Settings(SettingsDraft::open(&self.opts, in_name, out_name))
                    }
                };
            }
            Message::SettingsInput(choice) => {
                if let View::Settings(draft) = &mut self.view {
                    draft.input = choice;
                    // The tapped channel must exist on the new device.
                    if draft.in_channel > draft.input_channels() {
                        draft.in_channel = 1;
                    }
                }
            }
            Message::SettingsOutput(choice) => {
                if let View::Settings(draft) = &mut self.view {
                    draft.output = choice;
                }
            }
            Message::SettingsChannel(channel) => {
                if let View::Settings(draft) = &mut self.view {
                    draft.in_channel = channel;
                }
            }
            Message::SettingsBuffer(choice) => {
                if let View::Settings(draft) = &mut self.view {
                    draft.buffer = choice;
                }
            }
            // Handled at the App level; nothing to do if it ever lands here.
            Message::SettingsApply => {}
            Message::PresetNameChanged(name) => self.preset_name = name,
            Message::SavePreset => {
                let name = self.preset_name.trim().to_string();
                match self.session.save_preset(&name) {
                    Ok(msg) => {
                        self.status = msg;
                        self.active_preset = Some(name);
                        self.presets = list_presets();
                    }
                    Err(e) => self.status = format!("error: {e}"),
                }
            }
            Message::LoadPreset(name) => match self.session.load_preset(&name) {
                Ok(lines) => {
                    for line in &lines {
                        eprintln!("{line}");
                    }
                    self.status = lines.last().cloned().unwrap_or_default();
                    self.active_preset = Some(name.clone());
                    self.preset_name = name;
                    self.refresh_slots();
                }
                Err(e) => self.status = format!("error: {e}"),
            },
        }
    }

    /// Switch to a view tab. Radio semantics: re-clicking the active tab is
    /// a no-op (so it never resets the tuner window mid-reading).
    fn show_panel(&mut self, panel: Panel) {
        let already = matches!(
            (&self.view, panel),
            (View::Board, Panel::Board)
                | (View::Tuner, Panel::Tuner)
                | (View::Eq, Panel::Eq)
                | (View::Live, Panel::Live)
        );
        if already {
            return;
        }
        self.view = match panel {
            Panel::Board => View::Board,
            Panel::Eq => View::Eq,
            // Tuner runs in live mode too; stale tap contents would smear
            // the first estimate.
            Panel::Tuner | Panel::Live => {
                self.drain_tap();
                self.tuner.reset();
                self.reading = None;
                if panel == Panel::Tuner {
                    View::Tuner
                } else {
                    View::Live
                }
            }
        };
    }

    fn on_frame(&mut self, now: Instant) {
        if let Some(last) = self.last_frame {
            let dt = (now - last).as_secs_f32();
            self.frame_secs = 0.9 * self.frame_secs + 0.1 * dt;
        }
        self.last_frame = Some(now);
        self.frame_count = self.frame_count.wrapping_add(1);

        if self.frame_count.is_multiple_of(GC_FRAMES) {
            self.session.collect_garbage();
        }

        // Keep the preset bar's list fresh (~1 Hz) — files can appear or
        // vanish under ~/.lion-heart/presets while we run.
        if self.frame_count.is_multiple_of(PRESET_SCAN_FRAMES) {
            let fresh = list_presets();
            if fresh != self.presets {
                self.presets = fresh;
            }
        }

        // Foot controller: apply queued MIDI and mirror the result in the UI.
        let midi_lines = self.session.drain_midi();
        if !midi_lines.is_empty() {
            self.status = midi_lines.last().cloned().unwrap_or_default();
            self.refresh_slots();
            if self.session.config.last_preset != self.active_preset {
                self.active_preset = self.session.config.last_preset.clone();
                if let Some(name) = &self.active_preset {
                    self.preset_name = name.clone();
                }
            }
        }

        let telemetry = self.session.chain.telemetry();
        self.ballistics
            .tick(telemetry.peak_in(), telemetry.peak_out());
        self.meter_cache.clear();
        self.live_meter_cache.clear();

        // Keep the spectrum window fresh even while the panel is closed
        // (the tap is drop-on-full either way).
        if let Some(tap) = &mut self.spectrum_tap {
            let available = tap.slots();
            if available > 0
                && let Ok(chunk) = tap.read_chunk(available)
            {
                let (a, b) = chunk.as_slices();
                self.analyzer.feed(a);
                self.analyzer.feed(b);
                chunk.commit_all();
            }
        }
        if matches!(self.view, View::Eq) && self.frame_count.is_multiple_of(SPECTRUM_FRAMES) {
            self.analyzer.update();
            self.eq_cache.clear();
        }

        self.drain_tap();
        if matches!(self.view, View::Tuner | View::Live) {
            if self.frame_count.is_multiple_of(TUNER_FRAMES) {
                if let Some(est) = self.tuner.estimate() {
                    self.reading = Some((
                        Reading {
                            note: est.note_name().to_string(),
                            octave: est.octave(),
                            cents: est.cents(),
                            freq_hz: est.freq_hz,
                        },
                        now,
                    ));
                }
                if let Some((_, at)) = &self.reading
                    && now.duration_since(*at) > TUNER_HOLD
                {
                    self.reading = None;
                }
            }
            self.tuner_cache.clear();
        }
    }

    /// Move tapped input samples into the tuner's sliding window.
    fn drain_tap(&mut self) {
        let Some(tap) = &mut self.tap else { return };
        let available = tap.slots();
        if available == 0 {
            return;
        }
        if let Ok(chunk) = tap.read_chunk(available) {
            let (a, b) = chunk.as_slices();
            self.tuner.feed(a);
            self.tuner.feed(b);
            chunk.commit_all();
        }
    }

    fn move_slot(&mut self, key: &str, delta: isize) {
        let Some(pos) = self.slots.iter().position(|s| s.key == key) else {
            return;
        };
        let target = pos as isize + delta;
        if target < 0 || target as usize >= self.slots.len() {
            return;
        }
        match self.session.chain.move_position(pos, target as usize) {
            Ok(()) => {
                self.refresh_slots();
                self.select_position(target as usize);
                self.status = format!("chain: {}", self.session.chain.order_handles().join(" → "));
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Restart the audio session with the draft settings, carrying chain
    /// state and assets across. On failure the previous configuration is
    /// restored; only when that also fails is audio truly gone (→ Failed).
    fn apply_settings(self: Box<Self>) -> App {
        let mut this = *self;
        let View::Settings(draft) = &this.view else {
            return App::Running(Box::new(this));
        };
        let new_opts = draft.to_opts(&this.opts);
        if new_opts.input == this.opts.input
            && new_opts.output == this.opts.output
            && new_opts.in_channel == this.opts.in_channel
            && new_opts.buffer == this.opts.buffer
        {
            this.status = "settings unchanged".into();
            return App::Running(Box::new(this));
        }

        let carry = this.session.carry_over();
        let old_opts = this.opts.clone();
        // The old stream must release its devices before the new
        // configuration opens them — a buffer change on the same device
        // would otherwise conflict.
        drop(this.session);

        let (mut session, lines, applied) = match Session::resume(&new_opts, &carry) {
            Ok((session, lines)) => (session, lines, true),
            Err(e) => match Session::resume(&old_opts, &carry) {
                Ok((session, mut lines)) => {
                    lines.push(format!(
                        "settings not applied: {e} — previous configuration restored"
                    ));
                    (session, lines, false)
                }
                Err(rollback) => {
                    return App::Failed(format!(
                        "{e}\n\nrestoring the previous configuration also failed: {rollback}"
                    ));
                }
            },
        };
        for line in &lines {
            eprintln!("{line}");
        }

        if applied {
            this.opts = new_opts;
            session.remember_io(&this.opts);
            let (in_name, out_name) = session.io_names();
            this.status = format!(
                "audio restarted — {} → {} @ {} Hz, buffer {}",
                in_name,
                out_name,
                session.sample_rate,
                match this.opts.buffer {
                    Some(n) => n.to_string(),
                    None => "auto".into(),
                },
            );
            // A restore problem (e.g. an asset file gone) changes the tone —
            // it must not hide behind the success line.
            if let Some(issue) = lines
                .iter()
                .rev()
                .find(|l| l.starts_with("error:") || l.starts_with("warning:"))
            {
                this.status = format!("{} · {issue}", this.status);
            }
        } else {
            this.status = lines.last().cloned().unwrap_or_default();
        }

        // New stream ⇒ new taps, and possibly a new sample rate.
        this.tap = session.take_tuner_tap();
        this.tuner = Tuner::new(session.sample_rate);
        this.reading = None;
        this.spectrum_tap = session.take_spectrum_tap();
        this.analyzer = SpectrumAnalyzer::new(session.sample_rate);
        this.session = session;
        this.refresh_slots();
        App::Running(Box::new(this))
    }

    // --- view ---

    fn view(&self) -> Element<'_, Message> {
        let content = column![
            self.header(),
            self.preset_bar(),
            self.chain_strip(),
            self.main_panel(),
            self.footer(),
        ]
        .spacing(12)
        .padding(16);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::root)
            .into()
    }

    /// Wordmark, the view tabs as one grouped segmented control, and — set
    /// apart on the right, utility-corner style — settings and the meters.
    fn header(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, panel: Panel, lit: bool| {
            button(text(label).size(13))
                .padding([5, 13])
                .on_press(Message::ShowPanel(panel))
                .style(theme::chip(lit))
        };
        let tabs = container(
            row![
                tab("board", Panel::Board, matches!(self.view, View::Board)),
                tab("tuner", Panel::Tuner, matches!(self.view, View::Tuner)),
                tab("eq", Panel::Eq, matches!(self.view, View::Eq)),
                tab("live", Panel::Live, matches!(self.view, View::Live)),
            ]
            .spacing(2),
        )
        .padding(3)
        .style(theme::tab_group);
        row![
            row![
                text("LION").size(19).color(theme::TEXT_BRIGHT),
                text("-HEART").size(19).color(theme::ACCENT),
            ],
            tabs,
            space().width(Length::Fill),
            button(text("settings").size(13))
                .padding([5, 13])
                .on_press(Message::ToggleSettings)
                .style(theme::chip(matches!(self.view, View::Settings(_)))),
            Canvas::new(Meters {
                norms: self.ballistics.norms(),
                holds: self.ballistics.holds(),
                cache: &self.meter_cache,
            })
            .width(250)
            .height(40),
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// The persistent preset bar: step through presets, pick one directly,
    /// or save the current board under a name. Grows gracefully as presets
    /// accumulate (the picker scrolls); MIDI PC changes land here too.
    fn preset_bar(&self) -> Element<'_, Message> {
        let step = |label: &'static str, target: Option<String>| {
            button(text(label).size(12))
                .padding([4, 10])
                .on_press_maybe(target.map(Message::LoadPreset))
                .style(theme::action)
        };
        container(
            row![
                text("PRESET").size(11).color(theme::TEXT_DIM),
                step("◀", self.preset_neighbor(-1)),
                pick_list(
                    self.presets.clone(),
                    self.active_preset.clone(),
                    Message::LoadPreset,
                )
                .placeholder("— none saved —")
                .style(theme::pick)
                .menu_style(theme::menu)
                .text_size(13)
                .width(Length::Fixed(230.0)),
                step("▶", self.preset_neighbor(1)),
                space().width(Length::Fill),
                text_input("save as…", &self.preset_name)
                    .on_input(Message::PresetNameChanged)
                    .on_submit(Message::SavePreset)
                    .style(theme::input)
                    .size(13)
                    .width(Length::Fixed(190.0)),
                button(text("save").size(12))
                    .padding([4, 14])
                    .on_press(Message::SavePreset)
                    .style(theme::primary),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding([8, 12])
        .width(Length::Fill)
        .style(theme::panel)
        .into()
    }

    /// Families the "＋" menu offers right now (asset-mounting families stay
    /// singletons; nothing offered when the chain is full).
    fn addable_families(&self) -> Vec<&'static str> {
        if self.session.chain.is_full() {
            return Vec::new();
        }
        FAMILY_REGISTRY
            .iter()
            .filter(|e| !(e.asset.is_some() && self.session.chain.contains_family(e.desc.key)))
            .map(|e| e.desc.key)
            .collect()
    }

    /// The chain strip *is* the board editor (PRD 002): press-and-drag a
    /// card onto another to move it, click to select, "＋" appends a slot.
    /// Cards wear their pedal's identity color (stripe + pedal name); the
    /// LED dot is the bypass state; chevrons mark the signal flow.
    fn chain_strip(&self) -> Element<'_, Message> {
        let mut strip = row![].spacing(5).align_y(iced::Alignment::Center);
        let dragging = self.drag.as_ref().map(|d| d.from);
        let target = self.drag.as_ref().and_then(|d| d.over);
        for (position, slot) in self.slots.iter().enumerate() {
            if position > 0 {
                strip = strip.push(
                    text("›")
                        .size(15)
                        .color(theme::dim(theme::TEXT_DIM, 0.65)),
                );
            }
            let led = if slot.active {
                theme::METER_OK
            } else {
                theme::dim(theme::TEXT_DIM, 0.6)
            };
            let stripe = if slot.active {
                slot.color
            } else {
                theme::dim(slot.color, 0.35)
            };
            let mut card = column![
                container(space().width(Length::Fill).height(Length::Fixed(3.0)))
                    .style(theme::identity_rule(stripe)),
                row![text("●").size(8).color(led), text(slot.name).size(13)]
                    .spacing(5)
                    .align_y(iced::Alignment::Center),
            ]
            .spacing(4)
            .align_x(iced::Alignment::Center);
            card = card.push(if slot.pedals.len() > 1 {
                text(slot.pedals[slot.active_pedal].name)
                    .size(10)
                    .color(if slot.active {
                        slot.color
                    } else {
                        theme::TEXT_DIM
                    })
            } else {
                text(if slot.active { "on" } else { "bypassed" })
                    .size(10)
                    .color(theme::TEXT_DIM)
            });
            strip = strip.push(
                mouse_area(container(card).width(Length::Fill).padding([6, 5]).style(
                    theme::drag_card(
                        slot.color,
                        slot.key == self.selected,
                        slot.active,
                        dragging == Some(position),
                        target == Some(position),
                    ),
                ))
                .on_press(Message::CardPress(position))
                .on_enter(Message::CardEnter(position))
                .on_exit(Message::CardExit(position))
                .on_release(Message::CardRelease(position)),
            );
        }
        if self.slots.is_empty() {
            strip = strip.push(
                text("empty board (passthrough) — add a pedal with ＋")
                    .size(13)
                    .color(theme::TEXT_DIM),
            );
        }
        let addable = self.addable_families();
        if !addable.is_empty() {
            strip = strip.push(
                pick_list(addable, None::<&'static str>, Message::AddSlot)
                    .placeholder("＋")
                    .style(theme::pick)
                    .menu_style(theme::menu)
                    .text_size(14)
                    .width(Length::Fixed(64.0)),
            );
        }
        strip.into()
    }

    fn main_panel(&self) -> Element<'_, Message> {
        match &self.view {
            View::Browser(browser) => browser.view(),
            View::Tuner => container(
                Canvas::new(TunerDisplay {
                    reading: self.reading.as_ref().map(|(reading, _)| reading.clone()),
                    cache: &self.tuner_cache,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .style(theme::panel)
            .padding(8)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            View::Live => self.live_view(),
            View::Eq => self.eq_view(),
            View::Settings(draft) => self.settings_view(draft),
            View::Board => self.params_panel(),
        }
    }

    /// Global output EQ (PRD 003): the spectrum-backed band editor plus a
    /// detail strip for the selected band.
    fn eq_view(&self) -> Element<'_, Message> {
        let state = self.session.eq_state();
        let band = state.bands[self.eq_selected];

        let panel = Canvas::new(EqPanel {
            state,
            selected: self.eq_selected,
            spectrum: &self.analyzer.bins,
            sample_rate: self.analyzer.sample_rate(),
            cache: &self.eq_cache,
        })
        .width(Length::Fill)
        .height(Length::Fill);

        let mut toggled = band;
        toggled.enabled = !band.enabled;
        let readout = if band.kind.has_gain() {
            format!(
                "{:.0} Hz  {:+.1} dB  Q {:.2}",
                band.freq, band.gain_db, band.q
            )
        } else {
            format!("{:.0} Hz  Q {:.2}", band.freq, band.q)
        };
        let controls = row![
            text(format!("band {}", self.eq_selected + 1))
                .size(13)
                .color(theme::TEXT_BRIGHT),
            pick_list(BandKind::ALL, Some(band.kind), Message::EqKind)
                .style(theme::pick)
                .menu_style(theme::menu)
                .text_size(13),
            button(text(if band.enabled { "ON" } else { "OFF" }).size(12))
                .on_press(Message::EqBand {
                    index: self.eq_selected,
                    band: toggled,
                    commit: true,
                })
                .style(theme::chip(band.enabled)),
            text(readout)
                .size(13)
                .color(theme::TEXT_DIM)
                .font(iced::Font::MONOSPACE),
            space().width(Length::Fill),
            text("drag: freq/gain · wheel: Q · double-click: on/off")
                .size(11)
                .color(theme::TEXT_DIM),
            button(text("flat").size(12))
                .on_press(Message::EqFlat)
                .style(theme::action),
            button(
                text(if state.enabled {
                    "EQ ON"
                } else {
                    "EQ BYPASSED"
                })
                .size(12)
            )
            .on_press(Message::EqMaster)
            .style(theme::chip(state.enabled)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        container(column![panel, controls].spacing(10))
            .style(theme::panel)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Audio I/O settings: pick devices/channel/buffer, apply restarts the
    /// stream (chain state and assets carry over).
    fn settings_view<'a>(&'a self, draft: &'a SettingsDraft) -> Element<'a, Message> {
        let label = |s: &'static str| {
            text(s)
                .size(13)
                .color(theme::TEXT_DIM)
                .width(Length::Fixed(64.0))
        };
        let caps = |desc: Option<&DeviceDesc>,
                    dir: fn(&DeviceDesc) -> Option<&lh_io::devices::PortDesc>| {
            desc.and_then(dir)
                .map(|p| {
                    let buffers = match p.buffer_range {
                        Some((min, max)) => format!(" · buffer {min}–{max}"),
                        None => String::new(),
                    };
                    format!("{} ch @ {} Hz{}", p.channels, p.default_rate, buffers)
                })
                .unwrap_or_default()
        };
        let pick_row = |name, list, element: Element<'a, Message>| {
            row![
                label(name),
                element,
                text(list).size(12).color(theme::TEXT_DIM)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        };

        let input = pick_list(
            draft.input_options(),
            Some(draft.input.clone()),
            Message::SettingsInput,
        )
        .style(theme::pick)
        .menu_style(theme::menu)
        .text_size(13)
        .width(Length::Fixed(320.0));
        let output = pick_list(
            draft.output_options(),
            Some(draft.output.clone()),
            Message::SettingsOutput,
        )
        .style(theme::pick)
        .menu_style(theme::menu)
        .text_size(13)
        .width(Length::Fixed(320.0));
        let channel = pick_list(
            draft.channel_options(),
            Some(draft.in_channel),
            Message::SettingsChannel,
        )
        .style(theme::pick)
        .menu_style(theme::menu)
        .text_size(13)
        .width(Length::Fixed(90.0));
        let buffer = pick_list(
            draft.buffer_options(),
            Some(draft.buffer),
            Message::SettingsBuffer,
        )
        .style(theme::pick)
        .menu_style(theme::menu)
        .text_size(13)
        .width(Length::Fixed(180.0));

        let body = column![
            pick_row(
                "input",
                caps(draft.input_desc(), |d| d.input.as_ref()),
                input.into()
            ),
            pick_row("channel", String::new(), channel.into()),
            pick_row(
                "output",
                caps(draft.output_desc(), |d| d.output.as_ref()),
                output.into()
            ),
            pick_row("buffer", String::new(), buffer.into()),
            row![
                button(text("apply — restarts audio").size(13))
                    .on_press(Message::SettingsApply)
                    .style(theme::action),
                space().width(Length::Fill),
                button(text("close").size(12))
                    .on_press(Message::ToggleSettings)
                    .style(theme::action),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
            text("running:").size(12).color(theme::TEXT_DIM),
            text(self.session.description())
                .size(12)
                .color(theme::TEXT_DIM)
                .font(iced::Font::MONOSPACE),
        ]
        .spacing(12);

        container(scrollable(body))
            .style(theme::panel)
            .padding(14)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Stage mode: what you need mid-song, readable from a meter away.
    fn live_view(&self) -> Element<'_, Message> {
        let preset = self.active_preset.clone().unwrap_or_else(|| "—".into());
        let neighbor = |step: isize| self.preset_neighbor(step);
        let big_button = |label: &'static str, msg: Option<Message>| {
            button(text(label).size(26))
                .padding([14, 30])
                .on_press_maybe(msg)
                .style(theme::action)
        };

        let tuner_line = match &self.reading {
            Some((r, _)) => format!("{}{}  {:+.0}¢", r.note, r.octave, r.cents),
            None => String::new(),
        };
        let chain_line = self
            .slots
            .iter()
            .map(|s| {
                if s.active {
                    s.name.to_string()
                } else {
                    format!("({})", s.name)
                }
            })
            .collect::<Vec<_>>()
            .join(" → ");

        container(
            column![
                text("NOW PLAYING").size(12).color(theme::TEXT_DIM),
                text(preset).size(60).color(theme::ACCENT),
                row![
                    big_button("◀ prev", neighbor(-1).map(Message::LoadPreset)),
                    big_button("next ▶", neighbor(1).map(Message::LoadPreset)),
                ]
                .spacing(24),
                Canvas::new(Meters {
                    norms: self.ballistics.norms(),
                    holds: self.ballistics.holds(),
                    cache: &self.live_meter_cache,
                })
                .width(Length::Fixed(460.0))
                .height(70),
                text(tuner_line).size(22).color(theme::METER_OK),
                text(chain_line).size(13).color(theme::TEXT_DIM),
            ]
            .spacing(20)
            .align_x(iced::Alignment::Center),
        )
        .center(Length::Fill)
        .style(theme::panel)
        .into()
    }

    fn params_panel(&self) -> Element<'_, Message> {
        let Some(slot) = self.slots.iter().find(|s| s.key == self.selected) else {
            return container(
                column![
                    text("empty board").size(16).color(theme::TEXT_DIM),
                    text("add a pedal from the ＋ menu in the chain strip")
                        .size(13)
                        .color(theme::dim(theme::TEXT_DIM, 0.8)),
                ]
                .spacing(8)
                .align_x(iced::Alignment::Center),
            )
            .center(Length::Fill)
            .style(theme::panel)
            .into();
        };
        let pos = self
            .slots
            .iter()
            .position(|s| s.key == slot.key)
            .unwrap_or(0);
        let can_left = pos > 0;
        let can_right = pos + 1 < self.slots.len();

        let mut title = row![text(slot.name).size(17).color(theme::TEXT_BRIGHT)]
            .spacing(10)
            .align_y(iced::Alignment::Center);
        // Multi-pedal families pick their pedal right in the panel; the
        // knobs below re-render from the incoming pedal's own memory.
        if slot.pedals.len() > 1 {
            let names: Vec<&'static str> = slot.pedals.iter().map(|p| p.name).collect();
            let selected = slot.pedals[slot.active_pedal].name;
            let slot_key = slot.key.clone();
            title = title.push(
                pick_list(names, Some(selected), move |name: &'static str| {
                    Message::SelectPedal {
                        slot: slot_key.clone(),
                        pedal: name.to_string(),
                    }
                })
                .style(theme::pick)
                .menu_style(theme::menu)
                .text_size(13),
            );
        }
        title = title
            .push(
                button(text(if slot.active { "● ON" } else { "○ BYPASSED" }).size(12))
                    .padding([4, 12])
                    .on_press(Message::ToggleSlot(slot.key.clone()))
                    .style(theme::power(slot.active)),
            )
            .push(space().width(Length::Fill))
            .push(
                button(text("◀").size(12))
                    .padding([4, 10])
                    .on_press_maybe(can_left.then(|| Message::MoveSlotLeft(slot.key.clone())))
                    .style(theme::action),
            )
            .push(
                button(text("▶").size(12))
                    .padding([4, 10])
                    .on_press_maybe(can_right.then(|| Message::MoveSlotRight(slot.key.clone())))
                    .style(theme::action),
            )
            .push(
                button(text("remove").size(12))
                    .padding([4, 10])
                    .on_press(Message::RemoveSlot(slot.key.clone()))
                    .style(theme::danger),
            );

        // Stepped params (drive model, modulation type) pick from a
        // dropdown; the knob row below carries the continuous params.
        let mut selectors = row![].spacing(14);
        let mut has_selector = false;
        for param in &slot.params {
            let Some((labels, index)) = param.stepped else {
                continue;
            };
            has_selector = true;
            let slot_key = slot.key.clone();
            let param_key = param.key;
            let on_select = move |label: &'static str| {
                let i = labels.iter().position(|l| *l == label).unwrap_or(0);
                let norm = match labels.len() {
                    0 | 1 => 0.0,
                    n => i as f32 / (n - 1) as f32,
                };
                Message::Knob {
                    slot: slot_key.clone(),
                    param: param_key.to_string(),
                    norm,
                }
            };
            selectors = selectors.push(
                row![
                    text(param.name).size(13).color(theme::TEXT_DIM),
                    pick_list(labels, labels.get(index).copied(), on_select)
                        .style(theme::pick)
                        .menu_style(theme::menu)
                        .text_size(13),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        let mut knobs = row![].spacing(6);
        for (i, param) in slot
            .params
            .iter()
            .filter(|p| p.stepped.is_none())
            .enumerate()
            .take(MAX_KNOBS)
        {
            knobs = knobs.push(
                Canvas::new(knob::Knob {
                    slot: slot.key.clone(),
                    param: param.key.to_string(),
                    name: param.name,
                    value: param.display.clone(),
                    norm: param.norm,
                    default_norm: param.default_norm,
                    accent: slot.color,
                    cache: &self.knob_caches[i],
                })
                .width(knob::WIDTH)
                .height(knob::HEIGHT),
            );
        }

        // The pedal's identity rule separates its header from the controls.
        let rule = container(space().width(Length::Fill).height(Length::Fixed(2.0)))
            .style(theme::identity_rule(slot.color));

        let mut body = column![title, rule].spacing(12);
        if has_selector {
            body = body.push(selectors);
        }
        body = body.push(knobs);
        body = body.push(
            text("drag: set · wheel: nudge · double-click: default")
                .size(11)
                .color(theme::dim(theme::TEXT_DIM, 0.8)),
        );
        if let Some(kind) = crate::session::asset_kind(&slot.key) {
            let (nam_name, ir_name) = self.session.asset_names();
            let (label, file) = match kind {
                AssetKind::Nam => ("CAPTURE", nam_name),
                AssetKind::Ir => ("IMPULSE", ir_name),
            };
            let loaded = file != "-";
            body = body.push(
                container(
                    row![
                        text(label).size(10).color(theme::TEXT_DIM),
                        text(if loaded {
                            file
                        } else {
                            "— nothing loaded —".to_string()
                        })
                        .size(13)
                        .color(if loaded {
                            theme::TEXT_BRIGHT
                        } else {
                            theme::TEXT_DIM
                        })
                        .font(iced::Font::MONOSPACE),
                        space().width(Length::Fill),
                        button(text("load…").size(12))
                            .padding([3, 10])
                            .on_press(Message::OpenBrowser(kind))
                            .style(theme::action),
                        button(text("unload").size(12))
                            .padding([3, 10])
                            .on_press_maybe(loaded.then_some(Message::UnloadAsset(kind)))
                            .style(theme::danger),
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                )
                .padding([8, 12])
                .width(Length::Fill)
                .style(theme::inset),
            );
        }

        container(body)
            .style(theme::panel)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn footer(&self) -> Element<'_, Message> {
        let stats = self.session.stats();
        let fps = 1.0 / self.frame_secs.max(1e-4);
        let xruns = stats.underrun_events + stats.overrun_events;
        row![
            text(self.status.clone()).size(12).color(theme::TEXT_DIM),
            space().width(Length::Fill),
            // xruns only demand attention when they exist.
            text(format!("xruns {xruns}"))
                .size(12)
                .color(if xruns > 0 {
                    theme::METER_HOT
                } else {
                    theme::dim(theme::TEXT_DIM, 0.7)
                })
                .font(iced::Font::MONOSPACE),
            text(format!(
                "{fps:.0} fps · max cb {:.2} ms",
                stats.max_callback_millis(),
            ))
            .size(12)
            .color(theme::dim(theme::TEXT_DIM, 0.7))
            .font(iced::Font::MONOSPACE),
        ]
        .spacing(10)
        .into()
    }
}

fn format_value(real: f32, unit: &str) -> String {
    match unit {
        "Hz" if real >= 1000.0 => format!("{real:.0} {unit}"),
        "" => format!("{real:.2}"),
        _ => format!("{real:.1} {unit}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lh_io::devices::PortDesc;

    fn port(channels: u16) -> PortDesc {
        PortDesc {
            channels,
            default_rate: 48_000,
            min_rate: 44_100,
            max_rate: 96_000,
            sample_format: "f32".into(),
            buffer_range: Some((32, 4096)),
        }
    }

    fn device(index: usize, name: &str, ins: u16, outs: u16, default_io: bool) -> DeviceDesc {
        DeviceDesc {
            index,
            name: name.into(),
            is_default_input: default_io,
            is_default_output: default_io,
            input: (ins > 0).then(|| port(ins)),
            output: (outs > 0).then(|| port(outs)),
        }
    }

    fn opts(input: Option<&str>, buffer: Option<u32>, in_channel: u16) -> SessionOpts {
        SessionOpts {
            input: input.map(str::to_string),
            output: None,
            sample_rate: 48_000,
            buffer,
            in_channel,
            gain_db: -3.0,
            prefill_blocks: 2,
            tuner_tap: true,
            spectrum_tap: false,
            midi_port: None,
        }
    }

    fn devices() -> Vec<DeviceDesc> {
        vec![
            device(0, "Built-in Microphone", 1, 0, false),
            device(1, "Built-in Output", 0, 2, true),
            device(2, "Scarlett 2i2", 2, 2, false),
        ]
    }

    #[test]
    fn draft_mirrors_the_running_config_and_round_trips_to_opts() {
        // Explicit device: preselects the resolved name; default: stays
        // "system default" even though it resolved to a concrete device.
        let base = opts(Some("scarlett"), Some(128), 2);
        let draft =
            SettingsDraft::with_devices(devices(), &base, "Scarlett 2i2", "Built-in Output");
        assert_eq!(draft.input, DeviceChoice::Named("Scarlett 2i2".into()));
        assert_eq!(draft.output, DeviceChoice::Default);
        assert_eq!(draft.buffer, BufferChoice::Frames(128));

        let out = draft.to_opts(&base);
        assert_eq!(out.input.as_deref(), Some("Scarlett 2i2"));
        assert_eq!(out.output, None);
        assert_eq!(out.buffer, Some(128));
        assert_eq!(out.in_channel, 2);
        // Everything not on the panel is carried over from the base.
        assert_eq!(out.gain_db, -3.0);
        assert_eq!(out.prefill_blocks, 2);

        let auto = opts(None, None, 1);
        let draft = SettingsDraft::with_devices(devices(), &auto, "Built-in Microphone", "x");
        assert_eq!(draft.input, DeviceChoice::Default);
        assert_eq!(draft.buffer, BufferChoice::Auto);
        assert_eq!(draft.to_opts(&auto).buffer, None);
    }

    #[test]
    fn channel_options_follow_the_selected_input_device() {
        let base = opts(None, Some(64), 1);
        let mut draft =
            SettingsDraft::with_devices(devices(), &base, "Built-in Microphone", "Built-in Output");
        // Only devices with the right port direction are offered.
        assert_eq!(draft.input_options().len(), 1 + 2);
        assert_eq!(draft.output_options().len(), 1 + 2);
        // No default-input device in the list → fall back to one channel.
        assert_eq!(draft.channel_options(), vec![1]);

        draft.input = DeviceChoice::Named("Scarlett 2i2".into());
        assert_eq!(draft.channel_options(), vec![1, 2]);
    }

    #[test]
    fn nonstandard_buffer_sizes_stay_selectable() {
        let base = opts(None, Some(48), 1);
        let draft = SettingsDraft::with_devices(devices(), &base, "a", "b");
        let options = draft.buffer_options();
        assert_eq!(options[0], BufferChoice::Auto);
        let frames: Vec<u32> = options
            .iter()
            .filter_map(|o| match o {
                BufferChoice::Frames(n) => Some(*n),
                BufferChoice::Auto => None,
            })
            .collect();
        assert_eq!(frames, vec![32, 48, 64, 128, 256, 512, 1024]);

        // A standard value doesn't duplicate itself.
        let base = opts(None, Some(64), 1);
        let draft = SettingsDraft::with_devices(devices(), &base, "a", "b");
        assert_eq!(draft.buffer_options().len(), 7);
    }
}
