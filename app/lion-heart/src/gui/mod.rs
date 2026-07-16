//! The Lion-Heart GUI (M4 "the face"): run the pedalboard without a terminal.
//!
//! Threading contract (CLAUDE.md rules): this UI thread owns the [`Session`]
//! — `ChainHandle` for lock-free param/bypass/order messages, asset handles
//! for hot-swaps — and polls `Telemetry` atomics plus the tuner tap on every
//! window frame. Nothing here ever blocks or allocates on the audio thread;
//! retired assets are collected on the frame tick.

mod browser;
mod knob;
mod meter;
mod theme;
mod tuner;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Canvas};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, space, text, text_input,
};
use iced::{Element, Length, Size, Subscription, Theme, window};
use lh_dsp::tuner::Tuner;
use lh_io::devices::DeviceDesc;

use crate::cli::GuiArgs;
use crate::session::{Session, SessionOpts, list_presets, load_config};
use browser::Browser;
use meter::{Ballistics, Meters};
use tuner::{Reading, TunerDisplay};

/// Frames between retired-asset sweeps (~200 ms at 60 fps).
const GC_FRAMES: u32 = 12;
/// Frames between tuner estimates (~15 Hz at 60 fps).
const TUNER_FRAMES: u32 = 4;
/// Keep showing the last tuner reading this long after the note decays.
const TUNER_HOLD: Duration = Duration::from_millis(800);
const MAX_KNOBS: usize = 8;

pub fn run(args: GuiArgs) -> anyhow::Result<()> {
    iced::application(move || App::new(&args), App::update, App::view)
        .subscription(App::subscription)
        .theme(App::theme)
        .antialiasing(true)
        .window_size(Size::new(960.0, 580.0))
        .title("Lion-Heart")
        .run()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Nam,
    Ir,
}

#[derive(Debug, Clone)]
pub enum Message {
    Frame(Instant),
    SelectSlot(String),
    ToggleSlot(String),
    MoveSlotLeft(String),
    MoveSlotRight(String),
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
    TogglePresets,
    ToggleTuner,
    ToggleLive,
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

enum Overlay {
    None,
    Tuner,
    Presets,
    Browser(Browser),
    /// Stage mode: big preset name, prev/next, big meters.
    Live,
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
    key: String,
    name: &'static str,
    active: bool,
    params: Vec<ParamUi>,
}

struct ParamUi {
    key: &'static str,
    name: &'static str,
    norm: f32,
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
    slots: Vec<SlotUi>,
    selected: String,
    overlay: Overlay,
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
        let mut running = Running {
            session,
            opts,
            tap,
            tuner,
            reading: None,
            slots: Vec::new(),
            selected: String::new(),
            overlay: Overlay::None,
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
        let descs = self.session.chain.descriptors().to_vec();
        self.slots = self
            .session
            .chain
            .snapshot_chain()
            .iter()
            .map(|slot| {
                let desc = descs
                    .iter()
                    .find(|d| d.key == slot.key)
                    .expect("snapshot slots have descriptors");
                // The drive slot's knob captions follow the selected model
                // ("Gain" on a Blues Driver) — straight from the registry.
                let drive_model = (slot.key == "drive")
                    .then(|| slot.params.get("model").copied().unwrap_or(0.0) as usize);
                SlotUi {
                    key: slot.key.clone(),
                    name: desc.name,
                    active: slot.active,
                    params: desc
                        .params
                        .iter()
                        .map(|p| {
                            let real = slot.params.get(p.key).copied().unwrap_or(p.default);
                            let name = drive_model
                                .and_then(|m| lh_dsp::drive::model_knob_name(m, p.key))
                                .unwrap_or(p.name);
                            ParamUi {
                                key: p.key,
                                name,
                                norm: p.range.to_norm(real),
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

    fn update(&mut self, message: Message) {
        match message {
            Message::Frame(now) => self.on_frame(now),
            Message::SelectSlot(key) => {
                self.selected = key;
                for cache in &self.knob_caches {
                    cache.clear();
                }
            }
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
            Message::Knob { slot, param, norm } => {
                let real = self
                    .session
                    .chain
                    .descriptors()
                    .iter()
                    .find(|d| d.key == slot)
                    .and_then(|d| d.params.iter().find(|p| p.key == param))
                    .map(|p| p.range.to_real(norm));
                if let Some(real) = real {
                    match self.session.chain.set_param(&slot, &param, real) {
                        Ok(_) => self.refresh_slots(),
                        Err(e) => self.status = e.to_string(),
                    }
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
                self.overlay = Overlay::Browser(Browser::open(kind, start));
            }
            Message::BrowserNav(path) => {
                if let Overlay::Browser(browser) = &mut self.overlay {
                    browser.navigate(path);
                }
            }
            Message::BrowserPick(path) => {
                if let Overlay::Browser(browser) = &self.overlay {
                    let result = match browser.kind {
                        AssetKind::Nam => self.session.load_nam(&path),
                        AssetKind::Ir => self.session.load_ir(&path),
                    };
                    match result {
                        Ok(msg) => {
                            self.status = msg;
                            self.overlay = Overlay::None;
                        }
                        Err(e) => self.status = format!("error: {e}"),
                    }
                }
            }
            Message::BrowserClose => self.overlay = Overlay::None,
            Message::UnloadAsset(kind) => {
                let (had, label) = match kind {
                    AssetKind::Nam => (self.session.unload_nam(), "nam"),
                    AssetKind::Ir => (self.session.unload_ir(), "ir"),
                };
                if had {
                    self.status = format!("{label}: unloaded");
                }
            }
            Message::TogglePresets => {
                self.overlay = match self.overlay {
                    Overlay::Presets => Overlay::None,
                    _ => {
                        self.presets = list_presets();
                        Overlay::Presets
                    }
                };
            }
            Message::ToggleTuner => {
                self.overlay = match self.overlay {
                    Overlay::Tuner => Overlay::None,
                    _ => {
                        // Stale tap contents would smear the first estimate.
                        self.drain_tap();
                        self.tuner.reset();
                        self.reading = None;
                        Overlay::Tuner
                    }
                };
            }
            Message::ToggleLive => {
                self.overlay = match self.overlay {
                    Overlay::Live => Overlay::None,
                    _ => {
                        // The mini tuner readout runs in live mode too.
                        self.drain_tap();
                        self.tuner.reset();
                        self.reading = None;
                        Overlay::Live
                    }
                };
            }
            Message::ToggleSettings => {
                self.overlay = match self.overlay {
                    Overlay::Settings(_) => Overlay::None,
                    _ => {
                        let (in_name, out_name) = self.session.io_names();
                        Overlay::Settings(SettingsDraft::open(&self.opts, in_name, out_name))
                    }
                };
            }
            Message::SettingsInput(choice) => {
                if let Overlay::Settings(draft) = &mut self.overlay {
                    draft.input = choice;
                    // The tapped channel must exist on the new device.
                    if draft.in_channel > draft.input_channels() {
                        draft.in_channel = 1;
                    }
                }
            }
            Message::SettingsOutput(choice) => {
                if let Overlay::Settings(draft) = &mut self.overlay {
                    draft.output = choice;
                }
            }
            Message::SettingsChannel(channel) => {
                if let Overlay::Settings(draft) = &mut self.overlay {
                    draft.in_channel = channel;
                }
            }
            Message::SettingsBuffer(choice) => {
                if let Overlay::Settings(draft) = &mut self.overlay {
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

        self.drain_tap();
        if matches!(self.overlay, Overlay::Tuner | Overlay::Live) {
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
        if key == "limiter" {
            return;
        }
        let keys: Vec<String> = self.slots.iter().map(|s| s.key.clone()).collect();
        let Some(pos) = keys.iter().position(|k| k == key) else {
            return;
        };
        let target = pos as isize + delta;
        if target < 0 || target as usize >= keys.len() || keys[target as usize] == "limiter" {
            return;
        }
        let mut next = keys.clone();
        next.swap(pos, target as usize);
        let refs: Vec<&str> = next.iter().map(String::as_str).collect();
        match self.session.chain.set_order(&refs) {
            Ok(()) => {
                self.refresh_slots();
                self.status = format!("chain: {}", next.join(" → "));
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Restart the audio session with the draft settings, carrying chain
    /// state and assets across. On failure the previous configuration is
    /// restored; only when that also fails is audio truly gone (→ Failed).
    fn apply_settings(self: Box<Self>) -> App {
        let mut this = *self;
        let Overlay::Settings(draft) = &this.overlay else {
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

        // New stream ⇒ new tuner tap, and possibly a new sample rate.
        this.tap = session.take_tuner_tap();
        this.tuner = Tuner::new(session.sample_rate);
        this.reading = None;
        this.session = session;
        this.refresh_slots();
        App::Running(Box::new(this))
    }

    // --- view ---

    fn view(&self) -> Element<'_, Message> {
        let content = column![
            self.header(),
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

    fn header(&self) -> Element<'_, Message> {
        let preset_label = self.active_preset.as_deref().unwrap_or("presets");
        row![
            text("LION-HEART").size(18).color(theme::ACCENT),
            button(text(preset_label).size(13))
                .on_press(Message::TogglePresets)
                .style(theme::chip(matches!(self.overlay, Overlay::Presets))),
            button(text("tuner").size(13))
                .on_press(Message::ToggleTuner)
                .style(theme::chip(matches!(self.overlay, Overlay::Tuner))),
            button(text("live").size(13))
                .on_press(Message::ToggleLive)
                .style(theme::chip(matches!(self.overlay, Overlay::Live))),
            button(text("settings").size(13))
                .on_press(Message::ToggleSettings)
                .style(theme::chip(matches!(self.overlay, Overlay::Settings(_)))),
            space().width(Length::Fill),
            Canvas::new(Meters {
                norms: self.ballistics.norms(),
                cache: &self.meter_cache,
            })
            .width(240)
            .height(40),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn chain_strip(&self) -> Element<'_, Message> {
        let mut strip = row![].spacing(8);
        for slot in &self.slots {
            let state = if slot.active { "on" } else { "bypassed" };
            strip = strip.push(
                button(
                    column![
                        text(slot.name).size(14),
                        text(state).size(10).color(if slot.active {
                            theme::METER_OK
                        } else {
                            theme::TEXT_DIM
                        }),
                    ]
                    .spacing(2)
                    .align_x(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .padding([8, 4])
                .on_press(Message::SelectSlot(slot.key.clone()))
                .style(theme::slot_card(slot.key == self.selected, slot.active)),
            );
        }
        strip.into()
    }

    fn main_panel(&self) -> Element<'_, Message> {
        match &self.overlay {
            Overlay::Browser(browser) => browser.view(),
            Overlay::Presets => self.presets_view(),
            Overlay::Tuner => container(
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
            Overlay::Live => self.live_view(),
            Overlay::Settings(draft) => self.settings_view(draft),
            Overlay::None => self.params_panel(),
        }
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
        let neighbor = |step: isize| -> Option<String> {
            let current = self.active_preset.as_deref()?;
            let pos = self.presets.iter().position(|p| p == current)? as isize;
            let next = pos + step;
            (next >= 0 && (next as usize) < self.presets.len())
                .then(|| self.presets[next as usize].clone())
        };
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
                text(preset).size(54).color(theme::ACCENT),
                row![
                    big_button("◀ prev", neighbor(-1).map(Message::LoadPreset)),
                    big_button("next ▶", neighbor(1).map(Message::LoadPreset)),
                ]
                .spacing(24),
                Canvas::new(Meters {
                    norms: self.ballistics.norms(),
                    cache: &self.live_meter_cache,
                })
                .width(Length::Fixed(420.0))
                .height(70),
                text(tuner_line).size(22).color(theme::METER_OK),
                text(chain_line).size(13).color(theme::TEXT_DIM),
            ]
            .spacing(22)
            .align_x(iced::Alignment::Center),
        )
        .center(Length::Fill)
        .style(theme::panel)
        .into()
    }

    fn params_panel(&self) -> Element<'_, Message> {
        let Some(slot) = self.slots.iter().find(|s| s.key == self.selected) else {
            return container(text("no slot selected").color(theme::TEXT_DIM))
                .style(theme::panel)
                .padding(14)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        };
        let pos = self
            .slots
            .iter()
            .position(|s| s.key == slot.key)
            .unwrap_or(0);
        let movable = slot.key != "limiter";
        let can_left = movable && pos > 0;
        let can_right =
            movable && pos + 1 < self.slots.len() && self.slots[pos + 1].key != "limiter";

        let title = row![
            text(slot.name).size(16).color(theme::TEXT_BRIGHT),
            button(text(if slot.active { "ON" } else { "BYPASSED" }).size(12))
                .on_press(Message::ToggleSlot(slot.key.clone()))
                .style(theme::chip(slot.active)),
            space().width(Length::Fill),
            button(text("◀").size(12))
                .on_press_maybe(can_left.then(|| Message::MoveSlotLeft(slot.key.clone())))
                .style(theme::action),
            button(text("▶").size(12))
                .on_press_maybe(can_right.then(|| Message::MoveSlotRight(slot.key.clone())))
                .style(theme::action),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

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
                    cache: &self.knob_caches[i],
                })
                .width(knob::WIDTH)
                .height(knob::HEIGHT),
            );
        }

        let mut body = column![title].spacing(14);
        if has_selector {
            body = body.push(selectors);
        }
        body = body.push(knobs);
        if let Some(kind) = asset_kind_for(&slot.key) {
            let (nam_name, ir_name) = self.session.asset_names();
            let (label, file) = match kind {
                AssetKind::Nam => ("capture", nam_name),
                AssetKind::Ir => ("impulse", ir_name),
            };
            let loaded = file != "-";
            body = body.push(
                row![
                    text(format!("{label}: {file}"))
                        .size(13)
                        .color(if loaded {
                            theme::TEXT_BRIGHT
                        } else {
                            theme::TEXT_DIM
                        })
                        .font(iced::Font::MONOSPACE),
                    button(text("load…").size(12))
                        .on_press(Message::OpenBrowser(kind))
                        .style(theme::action),
                    button(text("unload").size(12))
                        .on_press_maybe(loaded.then_some(Message::UnloadAsset(kind)))
                        .style(theme::action),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );
        }

        container(body)
            .style(theme::panel)
            .padding(14)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn presets_view(&self) -> Element<'_, Message> {
        let save_row = row![
            text_input("preset name", &self.preset_name)
                .on_input(Message::PresetNameChanged)
                .on_submit(Message::SavePreset)
                .style(theme::input)
                .width(240),
            button(text("save").size(13))
                .on_press(Message::SavePreset)
                .style(theme::action),
            space().width(Length::Fill),
            button(text("close").size(12))
                .on_press(Message::TogglePresets)
                .style(theme::action),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let mut listing = column![].spacing(2);
        if self.presets.is_empty() {
            listing = listing.push(
                text("no presets yet — name one and press save")
                    .size(13)
                    .color(theme::TEXT_DIM),
            );
        }
        for name in &self.presets {
            let lit = self.active_preset.as_deref() == Some(name);
            listing = listing.push(
                button(text(name.clone()).size(13))
                    .width(Length::Fill)
                    .on_press(Message::LoadPreset(name.clone()))
                    .style(theme::chip(lit)),
            );
        }

        container(column![save_row, scrollable(listing).height(Length::Fill)].spacing(12))
            .style(theme::panel)
            .padding(14)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn footer(&self) -> Element<'_, Message> {
        let stats = self.session.stats();
        let fps = 1.0 / self.frame_secs.max(1e-4);
        row![
            text(self.status.clone()).size(12).color(theme::TEXT_DIM),
            space().width(Length::Fill),
            text(format!(
                "{fps:.0} fps · xruns {} · max cb {:.2} ms",
                stats.underrun_events + stats.overrun_events,
                stats.max_callback_millis(),
            ))
            .size(12)
            .color(theme::TEXT_DIM)
            .font(iced::Font::MONOSPACE),
        ]
        .spacing(10)
        .into()
    }
}

fn asset_kind_for(slot_key: &str) -> Option<AssetKind> {
    match slot_key {
        "amp" => Some(AssetKind::Nam),
        "cab" => Some(AssetKind::Ir),
        _ => None,
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
