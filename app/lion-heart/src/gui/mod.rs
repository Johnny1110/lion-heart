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
use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Length, Size, Subscription, Theme, window};
use lh_dsp::tuner::Tuner;

use crate::cli::GuiArgs;
use crate::session::{Session, SessionOpts, list_presets};
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
    PresetNameChanged(String),
    SavePreset,
    LoadPreset(String),
}

enum Overlay {
    None,
    Tuner,
    Presets,
    Browser(Browser),
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
}

enum App {
    Running(Box<Running>),
    /// Audio startup failed: show the error and the known fixes.
    Failed(String),
}

struct Running {
    session: Session,
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
    tuner_cache: canvas::Cache,
}

impl App {
    fn new(args: &GuiArgs) -> Self {
        let mut session = match Session::start(&SessionOpts {
            input: args.io.input.clone(),
            output: args.io.output.clone(),
            sample_rate: args.io.sample_rate,
            buffer: args.io.buffer_opt(),
            in_channel: args.io.in_channel,
            gain_db: args.gain_db,
            prefill_blocks: args.prefill_blocks,
            tuner_tap: true,
        }) {
            Ok(session) => session,
            Err(e) => return App::Failed(e.to_string()),
        };

        let mut status = session.description().to_string();
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
            tuner_cache: canvas::Cache::new(),
        };
        running.refresh_slots();
        App::Running(Box::new(running))
    }

    fn update(&mut self, message: Message) {
        if let App::Running(running) = self {
            running.update(message);
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
                SlotUi {
                    key: slot.key.clone(),
                    name: desc.name,
                    active: slot.active,
                    params: desc
                        .params
                        .iter()
                        .map(|p| {
                            let real = slot.params.get(p.key).copied().unwrap_or(p.default);
                            ParamUi {
                                key: p.key,
                                name: p.name,
                                norm: p.range.to_norm(real),
                                display: p
                                    .range
                                    .label(real)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| format_value(real, p.unit)),
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

        let telemetry = self.session.chain.telemetry();
        self.ballistics
            .tick(telemetry.peak_in(), telemetry.peak_out());
        self.meter_cache.clear();

        self.drain_tap();
        if matches!(self.overlay, Overlay::Tuner) {
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
            Overlay::None => self.params_panel(),
        }
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

        let mut knobs = row![].spacing(6);
        for (i, param) in slot.params.iter().enumerate().take(MAX_KNOBS) {
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

        let mut body = column![title, knobs].spacing(14);
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
