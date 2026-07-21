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
mod waveform;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Canvas};
use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, slider, space, text,
    text_input,
};
use iced::{Element, Length, Size, Subscription, Theme, window};
use lh_dsp::tuner::Tuner;
use lh_io::devices::DeviceDesc;

use crate::cli::GuiArgs;
pub use crate::session::AssetKind;
use crate::session::{
    FAMILY_REGISTRY, PresetInfo, Session, SessionOpts, list_presets, load_config, preset_info,
    save_preset_order,
};
use browser::Browser;
use eq::{EqPanel, EqTarget};
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
    /// Right-click on a knob: arm MIDI learn for it (or cancel, when it is
    /// the armed one) — the next CC binds and persists (PRD 008).
    MidiLearn {
        slot: String,
        param: String,
    },
    MidiLearnCancel,
    /// Clear the armed knob's CC binding (banner button).
    MidiClearBinding {
        slot: String,
        param: String,
    },
    /// Click a snapshot chip (PRD 009): switch to it if populated, else
    /// store the current scene into it.
    SnapshotChip(String),
    /// The ⤓ button: (over)write the active scene from the live board.
    SnapshotStore(String),
    /// Global tap-tempo (PRD 012): the footer BPM chip. `Tap` has no slot
    /// (the session-wide tempo); `TapTempo` is a delay faceplate's own TAP
    /// button — that slot's time is set even with `sync` off (the pre-012
    /// per-slot behavior, preserved).
    Tap,
    TapTempo(String),
    /// Toggle the practice metronome (PRD 019): the footer click chip.
    ToggleMetronome,
    /// Step the metronome's beats-per-bar through the common meters.
    CycleTimeSig,
    /// Toggle the practice drum groove (PRD 019 Phase 2): the footer drums chip.
    ToggleGroove,
    /// Step the drum groove through the built-in patterns.
    CycleGroove,
    /// Song player (PRD 019 Phase 3): transport + sliders.
    SongToggle,
    SongSeek(f32),
    SongSpeed(f32),
    SongTranspose(f32),
    SongMix(f32),
    /// Set the A-B loop start / end to the current position, or clear it.
    SongLoopA,
    SongLoopB,
    SongLoopClear,
    /// A looper transport momentary press (PRD 013): `action` is
    /// `"rec"`/`"undo"`/`"clear"`, fired as a 1.0→0.0 pulse by the session.
    LooperPress {
        slot: String,
        action: &'static str,
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
    /// Live band edit of a slot's parametric pedal on the board faceplate
    /// (PRD 011) — flows through the slot param path like a knob drag.
    PedalEqBand {
        slot: String,
        index: usize,
        band: Band,
    },
    PedalEqSelect(usize),
    /// Reset every band of a slot's parametric pedal to the transparent
    /// default layout.
    PedalEqFlat(String),
    ToggleSettings,
    SettingsInput(DeviceChoice),
    SettingsOutput(DeviceChoice),
    SettingsChannel(u16),
    SettingsBuffer(BufferChoice),
    /// Restart the audio session with the draft settings (handled at the
    /// [`App`] level: the running state is consumed and rebuilt). On the
    /// setup screen this is the "start audio" button.
    SettingsApply,
    /// Re-enumerate devices on the setup screen (interface plugged in
    /// after launch).
    SetupRescan,
    PresetNameChanged(String),
    SavePreset,
    LoadPreset(String),
    /// Preset management page (opened from the preset bar's "manage" button):
    /// list / save-as / load / rename / duplicate / delete.
    Preset(PresetMsg),
}

/// Messages for the preset management page. Nested under [`Message::Preset`]
/// to keep the whole CRUD surface in one place rather than a dozen flat
/// variants.
#[derive(Debug, Clone)]
pub enum PresetMsg {
    /// Open the management page (loads the on-disk digest).
    Open,
    /// Close it, back to the board.
    Close,
    /// The "save current board as…" field.
    NewNameChanged(String),
    SaveNew,
    /// Row press/hover/release for the click-to-load, drag-to-reorder gesture
    /// (same disambiguation the chain strip uses: release on the same row is a
    /// click → load; release on another row is a drag → reorder).
    RowPress(usize),
    RowEnter(usize),
    RowExit(usize),
    RowRelease(usize),
    /// Start a rename / duplicate edit on a row (seeds the input field).
    BeginRename(String),
    BeginDuplicate(String),
    /// The in-flight rename/duplicate input.
    EditChanged(String),
    CommitEdit,
    CancelEdit,
    /// Ask for / carry out / abandon a delete confirmation on a row.
    AskDelete(String),
    ConfirmDelete,
    CancelDelete,
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
    /// Practice song player: backing track + varispeed/transpose/loop (PRD 019).
    Song,
}

/// What the main panel is showing. [`Panel`] tabs cover the first four;
/// settings and the asset browser open from their own controls.
enum View {
    Board,
    Tuner,
    Eq,
    Live,
    /// Practice song player (PRD 019 Phase 3).
    Song,
    Browser(Browser),
    /// Audio I/O settings: devices, input channel, buffer size.
    Settings(SettingsDraft),
    /// Preset management: list every saved preset with CRUD + duplicate.
    Presets(PresetManager),
}

/// State for the preset management page (opened from the preset bar). Owns a
/// digest of every preset on disk plus one in-flight edit — a rename or
/// duplicate input, or a delete confirmation — and is rebuilt after every
/// mutation.
struct PresetManager {
    items: Vec<PresetInfo>,
    /// The "save current board as…" field.
    new_name: String,
    /// At most one row is mid-edit at a time.
    pending: Option<PendingEdit>,
    /// A row drag in progress (reuses the chain strip's [`DragState`]): press
    /// a row to start, release on itself to load it, on another to reorder.
    drag: Option<DragState>,
}

/// The single in-flight management action, if any. `target` is the preset the
/// action applies to; `input` is the new-name field for rename/duplicate.
enum PendingEdit {
    Rename { target: String, input: String },
    Duplicate { target: String, input: String },
    Delete { target: String },
}

impl PresetManager {
    /// Build the page state from the current on-disk preset set.
    fn load() -> Self {
        Self {
            items: list_presets().iter().map(|n| preset_info(n)).collect(),
            new_name: String::new(),
            pending: None,
            drag: None,
        }
    }

    /// Re-read the digests after a mutation, dropping any in-flight edit/drag.
    fn reload(&mut self) {
        self.items = list_presets().iter().map(|n| preset_info(n)).collect();
        self.pending = None;
        self.drag = None;
    }
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

/// The device / channel / buffer rows shared by the running settings panel
/// and the audio-down setup screen.
fn draft_controls<'a>(draft: &'a SettingsDraft) -> iced::widget::Column<'a, Message> {
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

    column![
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
    ]
    .spacing(12)
}

/// One chain slot as the UI sees it, rebuilt from the engine snapshot.
struct SlotUi {
    /// Instance handle ("drive", "drive2", …) — the engine address.
    key: String,
    /// Family key ("delay", "drive", …) — for family-specific UI (tap tempo).
    family_key: &'static str,
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
    /// Audio is not running (the saved interface is unplugged, startup
    /// failed, or a settings apply lost both configurations). The window
    /// stays up with a device picker so audio can be brought up without
    /// relaunching.
    Setup(Box<Setup>),
}

/// The audio-down state: why the stream is not running plus a device draft
/// to try again with. Applying here deliberately does **not** persist to
/// config.json — the saved preference (the usual interface) must win again
/// on the next launch; a temporary session on built-in devices is not a
/// change of setup. Persistent changes belong to the settings panel of a
/// running session.
struct Setup {
    error: String,
    draft: SettingsDraft,
    /// The options of the failed attempt; draft edits apply on top (gain,
    /// prefill, MIDI, and taps all carry over).
    opts: SessionOpts,
    /// Preset to load once audio comes up (CLI override or the session's
    /// last active one).
    preset: Option<String>,
}

impl Setup {
    fn new(error: String, opts: SessionOpts, preset: Option<String>) -> Self {
        let devices = lh_io::devices::enumerate().unwrap_or_default();
        // Preselect whatever the failed spec still resolves to; anything
        // missing (the unplugged interface) falls back to system default,
        // so a plain "start audio" always means something sensible.
        let resolved_in = lh_io::devices::resolve_name(
            &devices,
            opts.input.as_deref(),
            lh_io::devices::Direction::Input,
        );
        let resolved_out = lh_io::devices::resolve_name(
            &devices,
            opts.output.as_deref(),
            lh_io::devices::Direction::Output,
        );
        let probe = SessionOpts {
            input: resolved_in.clone(),
            output: resolved_out.clone(),
            ..opts.clone()
        };
        let mut draft = SettingsDraft::with_devices(
            devices,
            &probe,
            resolved_in.as_deref().unwrap_or(""),
            resolved_out.as_deref().unwrap_or(""),
        );
        if draft.in_channel > draft.input_channels() {
            draft.in_channel = 1;
        }
        Self {
            error,
            draft,
            opts,
            preset,
        }
    }

    /// Momentary value for `mem::replace` while a restart consumes the
    /// running state; never rendered.
    fn placeholder() -> Box<Self> {
        let opts = SessionOpts {
            input: None,
            output: None,
            sample_rate: 0,
            buffer: None,
            in_channel: 1,
            gain_db: 0.0,
            prefill_blocks: 1,
            tuner_tap: false,
            spectrum_tap: false,
            midi_port: None,
        };
        Box::new(Self {
            error: String::new(),
            draft: SettingsDraft::with_devices(Vec::new(), &opts, "", ""),
            opts,
            preset: None,
        })
    }

    /// Fresh device list, keeping the current choices where they survive.
    fn rescan(&mut self) {
        self.draft.devices = lh_io::devices::enumerate().unwrap_or_default();
        if self.draft.in_channel > self.draft.input_channels() {
            self.draft.in_channel = 1;
        }
    }

    /// Draft edits; everything session-bound is inert here.
    fn update(&mut self, message: Message) {
        match message {
            Message::SettingsInput(choice) => {
                self.draft.input = choice;
                if self.draft.in_channel > self.draft.input_channels() {
                    self.draft.in_channel = 1;
                }
            }
            Message::SettingsOutput(choice) => self.draft.output = choice,
            Message::SettingsChannel(channel) => self.draft.in_channel = channel,
            Message::SettingsBuffer(choice) => self.draft.buffer = choice,
            Message::SetupRescan => self.rescan(),
            _ => {}
        }
    }

    fn view(&self) -> Element<'_, Message> {
        container(
            column![
                row![
                    text("LION").size(19).color(theme::TEXT_BRIGHT),
                    text("-HEART").size(19).color(theme::ACCENT),
                ],
                text("audio is not running")
                    .size(20)
                    .color(theme::METER_HOT),
                container(
                    text(&self.error)
                        .size(13)
                        .color(theme::TEXT)
                        .font(iced::Font::MONOSPACE),
                )
                .padding([12, 14])
                .width(Length::Fill)
                .style(theme::inset),
                text("AUDIO I/O").size(11).color(theme::TEXT_DIM),
                draft_controls(&self.draft),
                row![
                    button(text("start audio").size(13))
                        .padding([6, 16])
                        .on_press(Message::SettingsApply)
                        .style(theme::primary),
                    button(text("rescan devices").size(12))
                        .padding([5, 12])
                        .on_press(Message::SetupRescan)
                        .style(theme::action),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                text(
                    "this choice is for this session only — the saved setup still \
                     wins on the next launch (change it in settings once audio is up)"
                )
                .size(11)
                .color(theme::dim(theme::TEXT_DIM, 0.9)),
            ]
            .spacing(14)
            .max_width(760),
        )
        .center(Length::Fill)
        .style(theme::root)
        .into()
    }
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
    /// Selected band of the board's parametric-pedal editor (PRD 011).
    pedal_eq_selected: usize,
    pedal_eq_cache: canvas::Cache,
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
        // No audio ⇒ no dead end: the setup screen owns recovery.
        match Running::start(opts.clone(), args.preset.clone()) {
            Ok(running) => App::Running(running),
            Err(error) => App::Setup(Box::new(Setup::new(error, opts, args.preset.clone()))),
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            // Applying settings consumes/creates the running state (the
            // session is torn down and rebuilt), so it is handled here at
            // the App level for both states.
            Message::SettingsApply => match self {
                App::Running(_) => {
                    let App::Running(running) =
                        std::mem::replace(self, App::Setup(Setup::placeholder()))
                    else {
                        unreachable!("matched App::Running above");
                    };
                    *self = running.apply_settings();
                }
                App::Setup(setup) => {
                    let opts = setup.draft.to_opts(&setup.opts);
                    match Running::start(opts.clone(), setup.preset.clone()) {
                        Ok(running) => *self = App::Running(running),
                        Err(error) => {
                            setup.error = error;
                            setup.opts = opts;
                            // The world may have changed since the last try
                            // (that is usually *why* it failed).
                            setup.rescan();
                        }
                    }
                }
            },
            message => match self {
                App::Running(running) => running.update(message),
                App::Setup(setup) => setup.update(message),
            },
        }
    }

    fn view(&self) -> Element<'_, Message> {
        match self {
            App::Running(running) => running.view(),
            App::Setup(setup) => setup.view(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        match self {
            App::Running(_) => window::frames().map(Message::Frame),
            App::Setup(_) => Subscription::none(),
        }
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

impl Running {
    /// Start the audio session and build the running state around it —
    /// shared by launch, the setup screen's "start audio", and (via
    /// [`Session::resume`]) the settings apply path. `Err` is the
    /// user-facing reason audio could not come up.
    fn start(opts: SessionOpts, preset_override: Option<String>) -> Result<Box<Self>, String> {
        let mut session = Session::start(&opts).map_err(|e| e.to_string())?;

        let mut status = format!("{} · {}", session.description(), session.midi_status);
        if let Some(name) = session.initial_preset(preset_override) {
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
            pedal_eq_selected: 2,
            pedal_eq_cache: canvas::Cache::new(),
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
        Ok(Box::new(running))
    }

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
                    family_key: family.key,
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
        self.pedal_eq_cache.clear();
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

    /// Apply one band of a slot's parametric pedal (PRD 011): diff against
    /// the UI mirror and send only the changed params, so a drag costs the
    /// same message traffic as a knob drag.
    fn apply_pedal_eq_band(&mut self, slot_key: &str, index: usize, band: Band) {
        use lh_dsp::eq::parametric::BAND_PARAMS;
        self.pedal_eq_selected = index;
        let band = band.clamped();
        let kind_index = BandKind::ALL
            .iter()
            .position(|k| *k == band.kind)
            .unwrap_or(0) as f32;
        let n = index + 1;
        let fields: [(String, f32); 5] = [
            (format!("b{n}_on"), if band.enabled { 1.0 } else { 0.0 }),
            (format!("b{n}_type"), kind_index),
            (format!("b{n}_freq"), band.freq),
            (format!("b{n}_gain"), band.gain_db),
            (format!("b{n}_q"), band.q),
        ];
        // The band's current mirror values, to skip no-ops.
        let current: Vec<f32> = self
            .slots
            .iter()
            .find(|s| s.key == slot_key)
            .map(|s| {
                let desc = s.pedals[s.active_pedal];
                s.params
                    .iter()
                    .zip(desc.params)
                    .skip(index * BAND_PARAMS)
                    .take(BAND_PARAMS)
                    .map(|(p, d)| d.range.to_real(p.norm))
                    .collect()
            })
            .unwrap_or_default();
        for (i, (key, real)) in fields.iter().enumerate() {
            let unchanged = current
                .get(i)
                .is_some_and(|c| (c - real).abs() <= 1e-3 * real.abs().max(1.0));
            if unchanged {
                continue;
            }
            match self.session.chain.set_param(slot_key, key, *real) {
                Ok(applied) => {
                    self.session.midi_desync_param(slot_key, key);
                    self.update_param_ui(slot_key, key, applied.real);
                }
                Err(e) => {
                    self.status = e.to_string();
                    return;
                }
            }
        }
        self.status = format!(
            "{} band {}: {} {:.0} Hz {:+.1} dB Q {:.2}{}",
            slot_key,
            n,
            band.kind,
            band.freq,
            band.gain_db,
            band.q,
            if band.enabled { "" } else { " (off)" },
        );
        self.pedal_eq_cache.clear();
    }

    /// Whether the board's selected slot currently shows the parametric
    /// pedal — its faceplate is the EQ canvas (PRD 011).
    fn selected_is_parametric(&self) -> bool {
        self.slots
            .iter()
            .find(|s| s.key == self.selected)
            .and_then(|s| s.pedals.get(s.active_pedal))
            .is_some_and(|p| p.key == "parametric")
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
            self.pedal_eq_cache.clear();
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
                        // The incoming pedal re-seats every param from its
                        // shadow: pickup-gated controllers must re-engage.
                        self.session.midi_desync_slot(&slot);
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
                    Ok(applied) if !stepped => {
                        // The knob moved out from under a pickup-gated pedal.
                        self.session.midi_desync_param(&slot, &param);
                        self.update_param_ui(&slot, &param, applied.real)
                    }
                    Ok(_) => {
                        self.session.midi_desync_param(&slot, &param);
                        self.refresh_slots();
                        // Moving a delay's subdivision, or its sync division,
                        // re-derives its time from the global tempo (ADR 014);
                        // refresh again if that moved the Time knob.
                        if (param == "subdivision" || param == "sync")
                            && self.session.apply_tempo_to(&slot)
                        {
                            self.refresh_slots();
                        }
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
            Message::MidiLearn { slot, param } => {
                // Right-click toggles: arming the armed knob cancels it.
                if self.session.midi_learn_target() == Some((slot.as_str(), param.as_str())) {
                    self.session.cancel_midi_learn();
                    self.status = "midi learn cancelled".into();
                } else {
                    match self.session.arm_midi_learn(&slot, &param) {
                        Ok(msg) => self.status = msg,
                        Err(e) => self.status = format!("midi learn: {e}"),
                    }
                }
                for cache in &self.knob_caches {
                    cache.clear(); // the armed badge moved
                }
            }
            Message::MidiLearnCancel => {
                if self.session.cancel_midi_learn() {
                    self.status = "midi learn cancelled".into();
                }
                for cache in &self.knob_caches {
                    cache.clear();
                }
            }
            Message::MidiClearBinding { slot, param } => {
                match self.session.clear_cc_binding(&slot, &param) {
                    Ok(msg) => self.status = msg,
                    Err(e) => self.status = format!("midi: {e}"),
                }
                self.session.cancel_midi_learn();
                for cache in &self.knob_caches {
                    cache.clear();
                }
            }
            Message::SnapshotChip(letter) => {
                // Populated → switch (may morph); empty → capture here.
                let populated = self
                    .session
                    .snapshot_chips()
                    .into_iter()
                    .any(|c| c.letter == letter && c.populated);
                let result = if populated {
                    self.session.switch_snapshot(&letter)
                } else {
                    self.session.store_snapshot(&letter)
                };
                match result {
                    Ok(msg) => self.status = msg,
                    Err(e) => self.status = e,
                }
                self.refresh_slots();
            }
            Message::SnapshotStore(letter) => match self.session.store_snapshot(&letter) {
                Ok(msg) => self.status = msg,
                Err(e) => self.status = e,
            },
            Message::Tap => {
                let msg = self.session.tap_tempo(None);
                if !msg.is_empty() {
                    self.status = msg;
                }
            }
            Message::TapTempo(slot) => {
                let msg = self.session.tap_tempo(Some(&slot));
                if !msg.is_empty() {
                    self.status = msg;
                }
            }
            Message::ToggleMetronome => {
                self.status = self.session.toggle_metronome();
            }
            Message::CycleTimeSig => {
                // Step through the common meters: 4 → 3 → 6 → 2 → 4…
                let next = match self.session.beats_per_bar() {
                    4 => 3,
                    3 => 6,
                    6 => 2,
                    _ => 4,
                };
                self.status = self.session.set_beats_per_bar(next);
            }
            Message::ToggleGroove => {
                self.status = self.session.toggle_groove();
            }
            Message::CycleGroove => {
                self.status = self.session.cycle_groove_pattern();
            }
            Message::SongToggle => {
                self.status = self.session.song_toggle();
            }
            Message::SongSeek(frac) => self.session.song_seek_fraction(frac),
            Message::SongSpeed(v) => {
                self.status = self.session.set_song_speed(v);
            }
            Message::SongTranspose(v) => {
                self.status = self.session.set_song_semitones(v);
            }
            Message::SongMix(v) => {
                self.status = self.session.set_song_mix(v);
            }
            Message::SongLoopA => {
                let (_, b) = self.session.song_loop_fraction().unwrap_or((0.0, 1.0));
                let a = self.session.song_fraction();
                self.status = self.session.set_song_loop_fraction(a, b.max(a));
            }
            Message::SongLoopB => {
                let (a, _) = self.session.song_loop_fraction().unwrap_or((0.0, 1.0));
                let b = self.session.song_fraction();
                self.status = self.session.set_song_loop_fraction(a.min(b), b);
            }
            Message::SongLoopClear => {
                self.status = self.session.clear_song_loop();
            }
            Message::LooperPress { slot, action } => {
                if let Err(e) = self.session.looper_press(&slot, action) {
                    self.status = e;
                }
                self.refresh_slots();
            }
            Message::OpenBrowser(kind) => {
                let remembered = match kind {
                    AssetKind::Nam => self.session.config.nam_dir.clone(),
                    AssetKind::Ir | AssetKind::IrB => self.session.config.ir_dir.clone(),
                    AssetKind::Song => self.session.config.song_dir.clone(),
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
                    let kind = browser.kind;
                    // A song loads on a background thread and lands in its own
                    // panel; the chain assets return to the board.
                    if kind == AssetKind::Song {
                        self.status = self.session.load_song(&path);
                        self.view = View::Song;
                    } else {
                        let result = match kind {
                            AssetKind::Nam => self.session.load_nam(&path),
                            AssetKind::Ir => self.session.load_ir(&path),
                            AssetKind::IrB => self.session.load_ir_b(&path),
                            AssetKind::Song => unreachable!(),
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
            }
            Message::BrowserClose => self.view = View::Board,
            Message::UnloadAsset(kind) => {
                let (had, label) = match kind {
                    AssetKind::Nam => (self.session.unload_nam(), "nam"),
                    AssetKind::Ir => (self.session.unload_ir(), "ir"),
                    AssetKind::IrB => (self.session.unload_ir_b(), "ir blend"),
                    // The song is not a chain asset — no unload routing.
                    AssetKind::Song => (false, "song"),
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
            Message::PedalEqBand { slot, index, band } => {
                self.apply_pedal_eq_band(&slot, index, band);
            }
            Message::PedalEqSelect(index) => {
                self.pedal_eq_selected = index;
                self.pedal_eq_cache.clear();
            }
            Message::PedalEqFlat(slot) => {
                for p in lh_dsp::eq::parametric::DESC.params {
                    if self
                        .session
                        .chain
                        .set_param(&slot, p.key, p.default)
                        .is_ok()
                    {
                        self.session.midi_desync_param(&slot, p.key);
                    }
                }
                self.refresh_slots();
                self.status = format!("{slot}: parametric reset to flat");
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
            // Handled at the App level / setup screen; nothing to do here.
            Message::SettingsApply | Message::SetupRescan => {}
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
            Message::LoadPreset(name) => self.apply_preset_load(name),
            Message::Preset(msg) => self.update_preset(msg),
        }
    }

    /// Load a preset by name and reflect it across the UI (shared by the
    /// preset bar's picker and the management page).
    fn apply_preset_load(&mut self, name: String) {
        match self.session.load_preset(&name) {
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
        }
    }

    /// Drive the preset management page. Every disk mutation is done through
    /// the session (so config/last-preset stay consistent), then the page's
    /// digest is rebuilt and the preset bar's list refreshed.
    fn update_preset(&mut self, msg: PresetMsg) {
        match msg {
            PresetMsg::Open => self.view = View::Presets(PresetManager::load()),
            PresetMsg::Close => self.view = View::Board,
            PresetMsg::RowPress(from) => {
                if let View::Presets(pm) = &mut self.view {
                    pm.drag = Some(DragState { from, over: None });
                }
            }
            PresetMsg::RowEnter(position) => {
                if let View::Presets(pm) = &mut self.view
                    && let Some(drag) = &mut pm.drag
                {
                    drag.over = (position != drag.from).then_some(position);
                }
            }
            PresetMsg::RowExit(position) => {
                if let View::Presets(pm) = &mut self.view
                    && let Some(drag) = &mut pm.drag
                    && drag.over == Some(position)
                {
                    drag.over = None;
                }
            }
            PresetMsg::RowRelease(to) => {
                // Resolve the gesture against the current drag, then drop the
                // view borrow before touching the session / other fields.
                enum Gesture {
                    Load(String),
                    Reorder(Vec<String>),
                    None,
                }
                let gesture = {
                    let View::Presets(pm) = &mut self.view else {
                        return;
                    };
                    let Some(drag) = pm.drag.take() else {
                        return;
                    };
                    if drag.from >= pm.items.len() || to >= pm.items.len() {
                        Gesture::None
                    } else if drag.from == to {
                        // Released where it started ⇒ a click: switch to it.
                        Gesture::Load(pm.items[to].name.clone())
                    } else {
                        let item = pm.items.remove(drag.from);
                        pm.items.insert(to, item);
                        Gesture::Reorder(pm.items.iter().map(|i| i.name.clone()).collect())
                    }
                };
                match gesture {
                    // Load but stay on the page, so presets can be auditioned
                    // one after another without leaving the manager.
                    Gesture::Load(name) => self.apply_preset_load(name),
                    Gesture::Reorder(order) => {
                        save_preset_order(&order);
                        self.presets = list_presets();
                        self.status = "preset order updated".into();
                    }
                    Gesture::None => {}
                }
            }
            PresetMsg::NewNameChanged(name) => {
                if let View::Presets(pm) = &mut self.view {
                    pm.new_name = name;
                }
            }
            PresetMsg::SaveNew => {
                let View::Presets(pm) = &self.view else {
                    return;
                };
                let name = pm.new_name.trim().to_string();
                match self.session.save_preset(&name) {
                    Ok(status) => {
                        self.status = status;
                        self.active_preset = Some(name.clone());
                        self.preset_name = name;
                        self.presets = list_presets();
                        if let View::Presets(pm) = &mut self.view {
                            pm.new_name.clear();
                            pm.reload();
                        }
                    }
                    Err(e) => self.status = format!("error: {e}"),
                }
            }
            PresetMsg::BeginRename(target) => {
                if let View::Presets(pm) = &mut self.view {
                    pm.pending = Some(PendingEdit::Rename {
                        input: target.clone(),
                        target,
                    });
                }
            }
            PresetMsg::BeginDuplicate(target) => {
                if let View::Presets(pm) = &mut self.view {
                    let input = format!("{target}-copy");
                    pm.pending = Some(PendingEdit::Duplicate { target, input });
                }
            }
            PresetMsg::EditChanged(text) => {
                if let View::Presets(pm) = &mut self.view {
                    match &mut pm.pending {
                        Some(PendingEdit::Rename { input, .. })
                        | Some(PendingEdit::Duplicate { input, .. }) => *input = text,
                        _ => {}
                    }
                }
            }
            PresetMsg::CommitEdit => {
                // Snapshot the edit (owned) before borrowing the session mutably.
                let edit = match &self.view {
                    View::Presets(pm) => match &pm.pending {
                        Some(PendingEdit::Rename { target, input }) => {
                            Some((true, target.clone(), input.trim().to_string()))
                        }
                        Some(PendingEdit::Duplicate { target, input }) => {
                            Some((false, target.clone(), input.trim().to_string()))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                let Some((is_rename, target, new)) = edit else {
                    return;
                };
                let result = if is_rename {
                    self.session.rename_preset(&target, &new)
                } else {
                    self.session.duplicate_preset(&target, &new)
                };
                match result {
                    Ok(status) => {
                        self.status = status;
                        // A rename of the active preset carries the pointer.
                        if is_rename && self.active_preset.as_deref() == Some(target.as_str()) {
                            self.active_preset = Some(new.clone());
                            self.preset_name = new;
                        }
                        self.presets = list_presets();
                        if let View::Presets(pm) = &mut self.view {
                            pm.pending = None;
                            pm.reload();
                        }
                    }
                    // Keep the editor open on error so the name can be fixed.
                    Err(e) => self.status = format!("error: {e}"),
                }
            }
            PresetMsg::CancelEdit | PresetMsg::CancelDelete => {
                if let View::Presets(pm) = &mut self.view {
                    pm.pending = None;
                }
            }
            PresetMsg::AskDelete(target) => {
                if let View::Presets(pm) = &mut self.view {
                    pm.pending = Some(PendingEdit::Delete { target });
                }
            }
            PresetMsg::ConfirmDelete => {
                let target = match &self.view {
                    View::Presets(pm) => match &pm.pending {
                        Some(PendingEdit::Delete { target }) => target.clone(),
                        _ => return,
                    },
                    _ => return,
                };
                match self.session.delete_preset(&target) {
                    Ok(status) => {
                        self.status = status;
                        if self.active_preset.as_deref() == Some(target.as_str()) {
                            self.active_preset = None;
                            self.preset_name.clear();
                        }
                        self.presets = list_presets();
                        if let View::Presets(pm) = &mut self.view {
                            pm.pending = None;
                            pm.reload();
                        }
                    }
                    Err(e) => self.status = format!("error: {e}"),
                }
            }
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
                | (View::Song, Panel::Song)
        );
        if already {
            return;
        }
        self.view = match panel {
            Panel::Board => View::Board,
            Panel::Eq => View::Eq,
            Panel::Song => View::Song,
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

        // Advance a snapshot morph, then reflect its live values on the
        // faceplate (knobs sweep with the scene change, PRD 009).
        let morphing = self.session.is_morphing();
        self.session.tick_morph(now);
        if morphing {
            self.refresh_slots();
        }

        // Re-derive tempo-locked controls (ADR 014); refresh the faceplate
        // only when a locked time/rate actually moved.
        if self.session.tick_tempo() {
            self.refresh_slots();
        }

        // A backing track finished decoding on its loader thread (PRD 019).
        if let Some(msg) = self.session.poll_song() {
            self.status = msg;
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
        // The EQ canvases (the global view, or a parametric faceplate on
        // the board) want live spectrum frames; skip the FFT when neither
        // is showing.
        let eq_canvas_open = matches!(self.view, View::Eq)
            || (matches!(self.view, View::Board) && self.selected_is_parametric());
        if eq_canvas_open && self.frame_count.is_multiple_of(SPECTRUM_FRAMES) {
            self.analyzer.update();
            self.eq_cache.clear();
            self.pedal_eq_cache.clear();
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
    /// restored; only when that also fails is audio truly gone — and then
    /// the setup screen takes over instead of a dead end.
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
                    // The chain-state snapshot is gone with the session, but
                    // recovery reloads the active preset, which rebuilds the
                    // board.
                    return App::Setup(Box::new(Setup::new(
                        format!(
                            "{e}\n\nrestoring the previous configuration also failed: {rollback}"
                        ),
                        old_opts,
                        this.active_preset.clone(),
                    )));
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
                tab("song", Panel::Song, matches!(self.view, View::Song)),
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
                button(text("manage").size(12))
                    .padding([4, 12])
                    .on_press(Message::Preset(PresetMsg::Open))
                    .style(theme::chip(matches!(self.view, View::Presets(_)))),
                space().width(Length::Fixed(18.0)),
                self.snapshot_chips_row(),
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

    /// The scene chips (PRD 009): A–D, click to switch (or store into an
    /// empty one); the active scene glows, populated ones read solid and
    /// empty ones dim, and a "*" marks unsaved drift. A ⤓ button beside the
    /// group re-captures the active scene.
    fn snapshot_chips_row(&self) -> Element<'_, Message> {
        let chips = self.session.snapshot_chips();
        let mut row = row![text("SCENE").size(11).color(theme::TEXT_DIM)]
            .spacing(5)
            .align_y(iced::Alignment::Center);
        for chip in &chips {
            let label = if chip.dirty {
                format!("{}*", chip.letter)
            } else {
                chip.letter.to_string()
            };
            let color = if chip.active {
                theme::ACCENT
            } else if chip.populated {
                theme::TEXT_BRIGHT
            } else {
                theme::dim(theme::TEXT_DIM, 0.7)
            };
            row = row.push(
                button(text(label).size(12).color(color))
                    .padding([3, 9])
                    .on_press(Message::SnapshotChip(chip.letter.to_string()))
                    .style(theme::chip(chip.active)),
            );
        }
        if let Some(active) = chips.iter().find(|c| c.active && c.populated) {
            let letter = active.letter.to_string();
            row = row.push(
                button(text("⤓").size(12))
                    .padding([3, 8])
                    .on_press(Message::SnapshotStore(letter))
                    .style(theme::action),
            );
        }
        row.into()
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
                strip = strip.push(text("›").size(15).color(theme::dim(theme::TEXT_DIM, 0.65)));
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
            View::Song => self.song_view(),
            View::Settings(draft) => self.settings_view(draft),
            View::Presets(pm) => self.presets_view(pm),
            View::Board => self.params_panel(),
        }
    }

    /// The practice song player (PRD 019 Phase 3): a backing track with
    /// varispeed (pitch-locked), transpose, an A-B loop, and a level. The
    /// waveform shows the playhead + loop region; the slider seeks.
    fn song_view(&self) -> Element<'_, Message> {
        let has = self.session.has_song();
        let playing = self.session.song_is_playing();
        let name = self.session.song_name().unwrap_or("(no backing track)");

        let pos = self.session.song_fraction();
        let total = self.session.song_seconds();
        let time = format!("{}  /  {}", clock(pos * total), clock(total));

        let transport = row![
            button(text(if playing { "■ stop" } else { "▶ play" }).size(14))
                .padding([6, 16])
                .on_press_maybe(has.then_some(Message::SongToggle))
                .style(theme::primary),
            button(text("load…").size(13))
                .padding([6, 14])
                .on_press(Message::OpenBrowser(AssetKind::Song))
                .style(theme::action),
            text(name).size(14).color(theme::TEXT_BRIGHT),
            space().width(Length::Fill),
            text(time)
                .size(13)
                .font(iced::Font::MONOSPACE)
                .color(theme::TEXT_DIM),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let wave = Canvas::new(waveform::Waveform {
            peaks: self.session.song_peaks(),
            position: pos,
            loop_range: self.session.song_loop_fraction(),
        })
        .width(Length::Fill)
        .height(Length::Fixed(120.0));

        let seek = slider(0.0f32..=1.0, pos, Message::SongSeek).step(0.001f32);

        let loop_label = match self.session.song_loop_fraction() {
            Some((a, b)) => format!("loop {}–{}", clock(a * total), clock(b * total)),
            None => "loop off".to_string(),
        };
        let loops = row![
            text(loop_label).size(13).color(theme::TEXT_DIM),
            space().width(Length::Fill),
            button(text("set A").size(12))
                .padding([4, 12])
                .on_press(Message::SongLoopA)
                .style(theme::action),
            button(text("set B").size(12))
                .padding([4, 12])
                .on_press(Message::SongLoopB)
                .style(theme::action),
            button(text("clear").size(12))
                .padding([4, 12])
                .on_press(Message::SongLoopClear)
                .style(theme::action),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        // Labelled sliders: varispeed, transpose, level.
        let speed = self.session.song_speed();
        let semis = self.session.song_semitones();
        let mix = self.session.song_mix();
        let label = |t: String| text(t).size(13).width(Length::Fixed(150.0));

        column![
            transport,
            wave,
            seek,
            loops,
            row![
                label(format!("speed  {:.0}%", speed * 100.0)),
                slider(0.25f32..=2.0, speed, Message::SongSpeed).step(0.01f32),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
            row![
                label(format!("transpose  {semis:+.0} st")),
                slider(-12.0f32..=12.0, semis, Message::SongTranspose).step(1.0f32),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
            row![
                label(format!("level  {:.0}%", mix * 100.0)),
                slider(0.0f32..=1.0, mix, Message::SongMix).step(0.01f32),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(4)
        .into()
    }

    /// The preset management page (opened from the preset bar). Lists every
    /// saved preset with load / rename / duplicate / delete, plus a "save the
    /// current board as a new preset" field. One row at a time can be mid-edit
    /// (rename/duplicate input) or awaiting a delete confirmation.
    fn presets_view<'a>(&self, pm: &'a PresetManager) -> Element<'a, Message> {
        let header = row![
            text("PRESETS").size(15).color(theme::TEXT_BRIGHT),
            text(format!("{} saved", pm.items.len()))
                .size(12)
                .color(theme::TEXT_DIM),
            space().width(Length::Fill),
            text("click a preset to switch · drag ⠿ to reorder")
                .size(11)
                .color(theme::TEXT_DIM),
            button(text("close").size(12))
                .padding([4, 12])
                .on_press(Message::Preset(PresetMsg::Close))
                .style(theme::action),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let can_save = !pm.new_name.trim().is_empty();
        let save_new = row![
            text("save current board as")
                .size(12)
                .color(theme::TEXT_DIM),
            text_input("new name…", &pm.new_name)
                .on_input(|s| Message::Preset(PresetMsg::NewNameChanged(s)))
                .on_submit(Message::Preset(PresetMsg::SaveNew))
                .style(theme::input)
                .size(13)
                .width(Length::Fixed(200.0)),
            button(text("save as new").size(12))
                .padding([4, 14])
                .on_press_maybe(can_save.then_some(Message::Preset(PresetMsg::SaveNew)))
                .style(theme::primary),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let mut list = column![].spacing(6);
        if pm.items.is_empty() {
            list = list.push(
                text("no presets yet — save the current board above")
                    .size(13)
                    .color(theme::TEXT_DIM),
            );
        }
        for (position, info) in pm.items.iter().enumerate() {
            list =
                list.push(self.preset_row(position, info, pm.pending.as_ref(), pm.drag.as_ref()));
        }

        let body = column![
            header,
            container(save_new)
                .style(theme::inset)
                .padding(10)
                .width(Length::Fill),
            scrollable(list).height(Length::Fill),
        ]
        .spacing(12);

        container(body)
            .style(theme::panel)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// One management row: a draggable body (grip + identity + chain digest)
    /// on the left, actions on the right. Pressing the body starts the
    /// click-to-load / drag-to-reorder gesture; the action buttons sit outside
    /// that zone so they stay independently clickable. When this row is the one
    /// mid-edit, the right side becomes the rename/duplicate field or the
    /// delete confirmation.
    fn preset_row<'a>(
        &self,
        position: usize,
        info: &'a PresetInfo,
        pending: Option<&'a PendingEdit>,
        drag: Option<&DragState>,
    ) -> Element<'a, Message> {
        let is_active = self.active_preset.as_deref() == Some(info.name.as_str());
        let name_color = if is_active {
            theme::ACCENT
        } else {
            theme::TEXT_BRIGHT
        };
        let dragged = drag.map(|d| d.from) == Some(position);
        let target = drag.and_then(|d| d.over) == Some(position);

        let mut meta = row![].spacing(6).align_y(iced::Alignment::Center);
        if let Some(err) = &info.error {
            meta = meta.push(text(format!("⚠ {err}")).size(11).color(theme::METER_HOT));
        } else {
            meta = meta.push(
                text(info.chain.clone())
                    .size(11)
                    .color(theme::TEXT_DIM)
                    .font(iced::Font::MONOSPACE),
            );
            if info.has_nam {
                meta = meta.push(Self::preset_badge("NAM"));
            }
            if info.has_ir {
                meta = meta.push(Self::preset_badge("IR"));
            }
            if info.scenes > 0 {
                let plural = if info.scenes == 1 { "" } else { "s" };
                meta = meta.push(Self::preset_badge(&format!(
                    "{} scene{plural}",
                    info.scenes
                )));
            }
        }

        let title = if is_active {
            format!("{}  · active", info.name)
        } else {
            info.name.clone()
        };
        let identity = column![text(title).size(14).color(name_color), meta]
            .spacing(3)
            .width(Length::Fill);

        // The grip + identity are the gesture zone: press to (potentially)
        // drag, release on self to load, release elsewhere to reorder.
        let body = mouse_area(
            row![
                text("⠿").size(14).color(theme::dim(theme::TEXT_DIM, 0.8)),
                identity,
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill),
        )
        .on_press(Message::Preset(PresetMsg::RowPress(position)));

        let right: Element<'a, Message> = match pending {
            Some(PendingEdit::Rename { target, input }) if target == &info.name => {
                Self::edit_affordance("rename to", input)
            }
            Some(PendingEdit::Duplicate { target, input }) if target == &info.name => {
                Self::edit_affordance("copy to", input)
            }
            Some(PendingEdit::Delete { target }) if target == &info.name => row![
                text("delete?").size(12).color(theme::METER_HOT),
                button(text("yes").size(12))
                    .padding([4, 12])
                    .on_press(Message::Preset(PresetMsg::ConfirmDelete))
                    .style(theme::danger),
                button(text("no").size(12))
                    .padding([4, 10])
                    .on_press(Message::Preset(PresetMsg::CancelDelete))
                    .style(theme::action),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .into(),
            _ => {
                let mut actions = row![].spacing(6).align_y(iced::Alignment::Center);
                // A broken file can only be removed — not renamed or copied
                // (loading it is still offered via the row click, which surfaces
                // the parse error).
                if info.error.is_none() {
                    actions = actions
                        .push(Self::preset_action(
                            "rename",
                            PresetMsg::BeginRename(info.name.clone()),
                        ))
                        .push(Self::preset_action(
                            "duplicate",
                            PresetMsg::BeginDuplicate(info.name.clone()),
                        ));
                }
                actions
                    .push(
                        button(text("delete").size(12))
                            .padding([4, 12])
                            .on_press(Message::Preset(PresetMsg::AskDelete(info.name.clone())))
                            .style(theme::danger),
                    )
                    .into()
            }
        };

        // The outer area spans the whole row, so hovering or releasing
        // anywhere on it tracks/commits a drag; the drop target frames in the
        // accent and the row being dragged mutes.
        mouse_area(
            container(
                row![body, right]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
            )
            .style(theme::preset_row(target, dragged))
            .padding([8, 12])
            .width(Length::Fill),
        )
        .on_enter(Message::Preset(PresetMsg::RowEnter(position)))
        .on_exit(Message::Preset(PresetMsg::RowExit(position)))
        .on_release(Message::Preset(PresetMsg::RowRelease(position)))
        .into()
    }

    /// The inline rename/duplicate editor: a name field plus commit/cancel.
    fn edit_affordance<'a>(label: &'static str, input: &'a str) -> Element<'a, Message> {
        row![
            text(label).size(11).color(theme::TEXT_DIM),
            text_input("name…", input)
                .on_input(|s| Message::Preset(PresetMsg::EditChanged(s)))
                .on_submit(Message::Preset(PresetMsg::CommitEdit))
                .style(theme::input)
                .size(13)
                .width(Length::Fixed(150.0)),
            button(text("ok").size(12))
                .padding([4, 12])
                .on_press(Message::Preset(PresetMsg::CommitEdit))
                .style(theme::primary),
            button(text("cancel").size(12))
                .padding([4, 10])
                .on_press(Message::Preset(PresetMsg::CancelEdit))
                .style(theme::action),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// A small dim pill (NAM / IR / scene count) for the digest line.
    fn preset_badge(label: &str) -> Element<'static, Message> {
        container(text(label.to_string()).size(10).color(theme::TEXT_DIM))
            .style(theme::inset)
            .padding([1, 6])
            .into()
    }

    /// A secondary row-action button wired to a [`PresetMsg`].
    fn preset_action(label: &'static str, msg: PresetMsg) -> Element<'static, Message> {
        button(text(label).size(12))
            .padding([4, 12])
            .on_press(Message::Preset(msg))
            .style(theme::action)
            .into()
    }

    /// Global output EQ (PRD 003): the spectrum-backed band editor plus a
    /// detail strip for the selected band.
    fn eq_view(&self) -> Element<'_, Message> {
        let state = self.session.eq_state().clone();
        let band = state.bands[self.eq_selected];
        let master_on = state.enabled;

        let panel = Canvas::new(EqPanel {
            state,
            target: EqTarget::Global,
            selected: self.eq_selected,
            spectrum: &self.analyzer.bins,
            spectrum_tag: None,
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
            button(text(if master_on { "EQ ON" } else { "EQ BYPASSED" }).size(12))
                .on_press(Message::EqMaster)
                .style(theme::chip(master_on)),
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

    /// The parametric pedal's faceplate (PRD 011): the shared EQ canvas
    /// bound to this slot's band params, plus the band detail strip. The
    /// spectrum overlay is the output-stage tap (tagged "OUT" — it is not a
    /// per-slot probe).
    fn pedal_eq_editor(&self, slot: &SlotUi) -> Element<'_, Message> {
        let desc = slot.pedals[slot.active_pedal];
        let reals: Vec<f32> = slot
            .params
            .iter()
            .zip(desc.params)
            .map(|(p, d)| d.range.to_real(p.norm))
            .collect();
        let bands = lh_dsp::eq::parametric::bands_from_reals(&reals);
        // `enabled` only tints the curve — actual bypass is the slot's LED
        // and engine crossfade, and a bypassed slot's faceplate still shows
        // its settings at full strength (knob rule).
        let state = lh_core::global_eq::GlobalEqState {
            enabled: true,
            bands,
        };
        let selected = self.pedal_eq_selected.min(bands.len() - 1);
        let band = state.bands[selected];

        let panel = Canvas::new(EqPanel {
            state,
            target: EqTarget::Slot(slot.key.clone()),
            selected,
            spectrum: &self.analyzer.bins,
            spectrum_tag: Some("OUT"),
            sample_rate: self.analyzer.sample_rate(),
            cache: &self.pedal_eq_cache,
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
        let kind_slot = slot.key.clone();
        let controls = row![
            text(format!("band {}", selected + 1))
                .size(13)
                .color(theme::TEXT_BRIGHT),
            pick_list(BandKind::ALL, Some(band.kind), move |kind| {
                let mut b = band;
                b.kind = kind;
                if !kind.has_gain() {
                    b.gain_db = 0.0;
                }
                Message::PedalEqBand {
                    slot: kind_slot.clone(),
                    index: selected,
                    band: b,
                }
            })
            .style(theme::pick)
            .menu_style(theme::menu)
            .text_size(13),
            button(text(if band.enabled { "ON" } else { "OFF" }).size(12))
                .on_press(Message::PedalEqBand {
                    slot: slot.key.clone(),
                    index: selected,
                    band: toggled,
                })
                .style(theme::chip(band.enabled)),
            text(readout)
                .size(13)
                .color(theme::TEXT_DIM)
                .font(iced::Font::MONOSPACE),
            space().width(Length::Fill),
            button(text("flat").size(12))
                .on_press(Message::PedalEqFlat(slot.key.clone()))
                .style(theme::action),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        column![panel, controls].spacing(10).into()
    }

    /// Audio I/O settings: pick devices/channel/buffer, apply restarts the
    /// stream (chain state and assets carry over).
    fn settings_view<'a>(&'a self, draft: &'a SettingsDraft) -> Element<'a, Message> {
        let body = column![
            text("AUDIO I/O").size(11).color(theme::TEXT_DIM),
            draft_controls(draft),
            row![
                button(text("apply — restarts audio").size(13))
                    .padding([6, 16])
                    .on_press(Message::SettingsApply)
                    .style(theme::primary),
                space().width(Length::Fill),
                button(text("close").size(12))
                    .padding([5, 12])
                    .on_press(Message::ToggleSettings)
                    .style(theme::action),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
            text("RUNNING").size(11).color(theme::TEXT_DIM),
            container(
                text(self.session.description())
                    .size(12)
                    .color(theme::TEXT)
                    .font(iced::Font::MONOSPACE),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(theme::inset),
        ]
        .spacing(14);

        container(scrollable(body))
            .style(theme::panel)
            .padding(16)
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
                button(
                    text(if slot.active {
                        "● ON"
                    } else {
                        "○ BYPASSED"
                    })
                    .size(12),
                )
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
            // Looper reverse/half are chips in the transport row (PRD 013).
            if slot.family_key == "looper" && matches!(param.key, "reverse" | "half") {
                continue;
            }
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

        let learn_target = self.session.midi_learn_target().map(|(s, p)| {
            (s.to_string(), p.to_string()) // owned: the borrow must not hold
        });
        let mut knobs = row![].spacing(6);
        for (i, param) in slot
            .params
            .iter()
            // Looper rec/undo/clear are momentary buttons in the transport row
            // below, not knobs (PRD 013).
            .filter(|p| {
                p.stepped.is_none()
                    && !(slot.family_key == "looper" && matches!(p.key, "rec" | "undo" | "clear"))
            })
            .enumerate()
            .take(MAX_KNOBS)
        {
            let learning = learn_target
                .as_ref()
                .is_some_and(|(s, p)| *s == slot.key && p == param.key);
            let midi = if learning {
                knob::MidiTag::Learning
            } else {
                match self.session.cc_binding(&slot.key, param.key) {
                    Some(cc) => knob::MidiTag::Mapped(cc),
                    None => knob::MidiTag::None,
                }
            };
            knobs = knobs.push(
                Canvas::new(knob::Knob {
                    slot: slot.key.clone(),
                    param: param.key.to_string(),
                    name: param.name,
                    value: param.display.clone(),
                    norm: param.norm,
                    default_norm: param.default_norm,
                    accent: slot.color,
                    midi,
                    cache: &self.knob_caches[i],
                })
                .width(knob::WIDTH)
                .height(knob::HEIGHT),
            );
        }

        // The pedal's identity rule separates its header from the controls.
        let rule = container(space().width(Length::Fill).height(Length::Fixed(2.0)))
            .style(theme::identity_rule(slot.color));

        let parametric = slot.pedals[slot.active_pedal].key == "parametric";
        let mut body = column![title, rule].spacing(12);
        if parametric {
            // The parametric pedal's faceplate is the EQ editor, not a knob
            // row (PRD 011) — same canvas as the global EQ view.
            body = body.push(self.pedal_eq_editor(slot));
        } else {
            if has_selector {
                body = body.push(selectors);
            }
            body = body.push(knobs);
        }
        // MIDI learn banner (PRD 008): armed target, cancel, clear-binding.
        if let Some((ls, lp)) = &learn_target {
            let mut banner = row![
                text(format!("MIDI learn: move a controller for {ls}.{lp}"))
                    .size(12)
                    .color(theme::ACCENT),
                button(text("cancel").size(11))
                    .padding([2, 10])
                    .on_press(Message::MidiLearnCancel)
                    .style(theme::action),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center);
            if let Some(cc) = self.session.cc_binding(ls, lp) {
                banner = banner.push(
                    button(text(format!("clear CC {cc}")).size(11))
                        .padding([2, 10])
                        .on_press(Message::MidiClearBinding {
                            slot: ls.clone(),
                            param: lp.clone(),
                        })
                        .style(theme::danger),
                );
            }
            body = body.push(banner);
        }
        body = body.push(
            text(if parametric {
                "drag: freq/gain · wheel: Q · double-click: band on/off"
            } else {
                "drag: set · wheel: nudge · double-click: default · right-click: midi learn"
            })
            .size(11)
            .color(theme::dim(theme::TEXT_DIM, 0.8)),
        );
        // Looper transport (PRD 013): rec/undo/clear are momentary buttons
        // (fired as a 1.0→0.0 pulse by the session), reverse/half are chips,
        // and a state LED (red = recording, green = playing, amber =
        // overdubbing) mirrors the effect's one-button state machine.
        if slot.family_key == "looper" {
            use crate::session::LooperLed;
            let (led_color, led_label) = match self.session.looper_led(&slot.key) {
                LooperLed::Empty => (theme::dim(theme::TEXT_DIM, 0.7), "empty"),
                LooperLed::Recording => (theme::METER_HOT, "recording"),
                LooperLed::Playing => (theme::METER_OK, "playing"),
                LooperLed::Overdubbing => (theme::ACCENT, "overdubbing"),
            };
            let stepped_on = |key: &str| -> bool {
                slot.params
                    .iter()
                    .find(|p| p.key == key)
                    .and_then(|p| p.stepped)
                    .is_some_and(|(_, i)| i == 1)
            };
            let reverse_on = stepped_on("reverse");
            let half_on = stepped_on("half");
            body = body.push(
                row![
                    row![
                        text("●").size(14).color(led_color),
                        text(led_label)
                            .size(13)
                            .color(theme::TEXT_DIM)
                            .font(iced::Font::MONOSPACE),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
                    space().width(Length::Fill),
                    button(text("⏺ REC").size(13))
                        .padding([8, 18])
                        .on_press(Message::LooperPress {
                            slot: slot.key.clone(),
                            action: "rec",
                        })
                        .style(theme::action),
                    button(text("↶ UNDO").size(13))
                        .padding([8, 16])
                        .on_press(Message::LooperPress {
                            slot: slot.key.clone(),
                            action: "undo",
                        })
                        .style(theme::action),
                    button(text("✕ CLEAR").size(13))
                        .padding([8, 16])
                        .on_press(Message::LooperPress {
                            slot: slot.key.clone(),
                            action: "clear",
                        })
                        .style(theme::danger),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );
            body = body.push(
                row![
                    button(text("reverse").size(12))
                        .padding([6, 14])
                        .on_press(Message::Knob {
                            slot: slot.key.clone(),
                            param: "reverse".to_string(),
                            norm: if reverse_on { 0.0 } else { 1.0 },
                        })
                        .style(theme::chip(reverse_on)),
                    button(text("half").size(12))
                        .padding([6, 14])
                        .on_press(Message::Knob {
                            slot: slot.key.clone(),
                            param: "half".to_string(),
                            norm: if half_on { 0.0 } else { 1.0 },
                        })
                        .style(theme::chip(half_on)),
                    text("½-speed = octave down · reverse = play backward")
                        .size(11)
                        .color(theme::dim(theme::TEXT_DIM, 0.8)),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );
        }
        // Tap tempo (PRD 004 / ADR 014): a momentary button, not a knob —
        // timed on the control side, it sets the rig's global tempo and (via
        // this slot's subdivision) a *Free* delay's own `time`. The `sync`
        // division dropdown sits in the selector row above; on anything but
        // Free, this slot's time locks to the global tempo instead.
        if slot.family_key == "delay" {
            let bpm = format!("♩ = {:.0} bpm", self.session.tempo_bpm());
            body = body.push(
                row![
                    button(text("TAP").size(15))
                        .padding([8, 26])
                        .on_press(Message::TapTempo(slot.key.clone()))
                        .style(theme::action),
                    text(bpm)
                        .size(13)
                        .color(theme::TEXT_DIM)
                        .font(iced::Font::MONOSPACE),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            );
        }
        if let Some(kind) = crate::session::asset_kind(&slot.key) {
            let (nam_name, ir_name) = self.session.asset_names();
            match kind {
                AssetKind::Nam => body = body.push(self.asset_row("CAPTURE", nam_name, kind)),
                AssetKind::Ir => {
                    // The cab shows both mics; its `blend` knob crosses A⇄B
                    // (ADR 015). Mic B is optional — load it to blend.
                    body = body.push(self.asset_row("MIC A", ir_name, AssetKind::Ir));
                    body = body.push(self.asset_row(
                        "MIC B",
                        self.session.ir_b_name(),
                        AssetKind::IrB,
                    ));
                }
                AssetKind::IrB => {}  // never a family mount
                AssetKind::Song => {} // never a family mount
            }
        }

        container(body)
            .style(theme::panel)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// One loaded-asset "LCD" row: label, file name, and load/unload buttons.
    fn asset_row(
        &self,
        label: &'static str,
        file: String,
        kind: AssetKind,
    ) -> Element<'_, Message> {
        let loaded = file != "-";
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
        .style(theme::inset)
        .into()
    }

    fn footer(&self) -> Element<'_, Message> {
        let stats = self.session.stats();
        let fps = 1.0 / self.frame_secs.max(1e-4);
        let xruns = stats.underrun_events + stats.overrun_events;
        let bpm = format!("♩ {:.0}", self.session.tempo_bpm());
        let metro_on = self.session.metronome_on();
        let sig = format!("{}/4", self.session.beats_per_bar());
        let groove_on = self.session.groove_on();
        let groove = self.session.groove_pattern_name().to_string();
        row![
            // The global tempo (ADR 014): click to tap. Delay faceplates
            // have their own TAP button; this is the one always in view.
            button(text(bpm).size(12).font(iced::Font::MONOSPACE))
                .padding([2, 10])
                .on_press(Message::Tap)
                .style(theme::action),
            // Practice metronome (PRD 019): lit amber when running; the
            // time-sig chip steps the accent's bar length.
            button(text("click").size(12).font(iced::Font::MONOSPACE))
                .padding([2, 10])
                .on_press(Message::ToggleMetronome)
                .style(theme::chip(metro_on)),
            button(text(sig).size(12).font(iced::Font::MONOSPACE))
                .padding([2, 8])
                .on_press(Message::CycleTimeSig)
                .style(theme::chip(false)),
            // Practice drum groove (PRD 019 Phase 2): lit when playing; the
            // pattern chip steps rock → funk → metal → ballad.
            button(text("drums").size(12).font(iced::Font::MONOSPACE))
                .padding([2, 10])
                .on_press(Message::ToggleGroove)
                .style(theme::chip(groove_on)),
            button(text(groove).size(12).font(iced::Font::MONOSPACE))
                .padding([2, 8])
                .on_press(Message::CycleGroove)
                .style(theme::chip(false)),
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

/// `m:ss` clock string for a duration in seconds (song player readouts).
fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
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
