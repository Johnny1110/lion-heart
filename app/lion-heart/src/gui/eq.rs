//! The 8-band EQ editor canvas: a log-frequency panel with two overlaid
//! curves — the EQ's composite response (setting) and the live output
//! spectrum (playing) — plus draggable band handles.
//!
//! Two targets share it (PRD 011): the **global** output EQ (PRD 003) and
//! any chain slot whose active pedal is the `parametric` — the same canvas,
//! publishing either the global messages or the slot-param ones.
//!
//! Interactions: drag a handle for freq/gain (cut bands: freq only), wheel
//! over it for Q, double-click to enable/disable. The detail strip below
//! the canvas (in `gui::mod`) covers type changes and numeric readouts.

use std::time::{Duration, Instant};

use iced::widget::canvas;
use iced::widget::text::Alignment as TextAlign;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Theme, mouse};
use lh_core::global_eq::{Band, FREQ_MAX, FREQ_MIN, GAIN_DB_MAX, GlobalEqState, Q_MAX, Q_MIN};

use super::Message;
use super::spectrum::DB_FLOOR;
use super::theme::{ACCENT, METER_OK, PANEL_HI, TEXT_BRIGHT, TEXT_DIM, TRACK};

const HIT_RADIUS: f32 = 14.0;
const HANDLE_RADIUS: f32 = 7.0;
const CURVE_POINTS: usize = 160;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Wheel sensitivity: Q multiplier per scroll line.
const Q_STEP: f32 = 1.12;

/// Who owns the bands this panel edits.
#[derive(Debug, Clone, PartialEq)]
pub enum EqTarget {
    /// The app-global output EQ — edits persist to `global_eq.json`.
    Global,
    /// A chain slot's parametric pedal (by instance handle) — edits flow
    /// through the slot param path like knob drags (PRD 011).
    Slot(String),
}

pub struct EqPanel<'a> {
    pub state: GlobalEqState,
    pub target: EqTarget,
    pub selected: usize,
    /// Display bins from the spectrum analyzer (dBFS).
    pub spectrum: &'a [f32],
    /// Corner note for the spectrum ("OUT" on slot panels: the tap sits on
    /// the output stage, not at the slot).
    pub spectrum_tag: Option<&'static str>,
    pub sample_rate: f32,
    pub cache: &'a canvas::Cache,
}

/// Pointer travel below this (px) is a click, not a drag.
const DRAG_SLOP: f32 = 3.0;
/// Shift-drag travel multiplier.
const FINE_DRAG: f32 = 0.25;

#[derive(Default)]
pub struct State {
    drag: Option<usize>,
    /// Press position, for the [`DRAG_SLOP`] test.
    press_at: Option<Point>,
    /// Dragged band's position in panel coordinates, accumulated from pointer travel
    /// and clamped to the panel.
    anchor: Option<Point>,
    /// Previous pointer position, for the travel delta.
    last: Option<Point>,
    /// Set once this press has cleared [`DRAG_SLOP`].
    moved: bool,
    /// Latched from keyboard events: mouse events carry no modifier state.
    modifiers: iced::keyboard::Modifiers,
    last_click: Option<(Instant, usize)>,
}

fn x_of_freq(width: f32, freq: f32) -> f32 {
    width * (freq / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln()
}

fn freq_of_x(width: f32, x: f32) -> f32 {
    (FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf((x / width).clamp(0.0, 1.0))).clamp(FREQ_MIN, FREQ_MAX)
}

/// Gain axis: ±GAIN_DB_MAX maps to the middle of the panel with headroom.
fn y_of_gain(height: f32, db: f32) -> f32 {
    height / 2.0 - (db / GAIN_DB_MAX) * (height / 2.0 - 14.0)
}

fn gain_of_y(height: f32, y: f32) -> f32 {
    (-(y - height / 2.0) / (height / 2.0 - 14.0) * GAIN_DB_MAX).clamp(-GAIN_DB_MAX, GAIN_DB_MAX)
}

/// Spectrum axis: DB_FLOOR..0 dBFS across the full height.
fn y_of_spectrum(height: f32, db: f32) -> f32 {
    height * (db / DB_FLOOR).clamp(0.0, 1.0)
}

impl EqPanel<'_> {
    /// The band-edit message for this panel's target. `commit` marks a
    /// persist point for the global EQ; the slot path applies live either
    /// way (preset saving is explicit).
    fn band_msg(&self, index: usize, band: Band, commit: bool) -> Message {
        match &self.target {
            EqTarget::Global => Message::EqBand {
                index,
                band,
                commit,
            },
            EqTarget::Slot(slot) => Message::PedalEqBand {
                slot: slot.clone(),
                index,
                band,
            },
        }
    }

    fn select_msg(&self, index: usize) -> Message {
        match &self.target {
            EqTarget::Global => Message::EqSelect(index),
            EqTarget::Slot(_) => Message::PedalEqSelect(index),
        }
    }

    fn handle_position(&self, size: iced::Size, band: usize) -> Point {
        let b = self.state.bands[band];
        let gain = if b.kind.has_gain() { b.gain_db } else { 0.0 };
        Point::new(x_of_freq(size.width, b.freq), y_of_gain(size.height, gain))
    }

    fn hit_test(&self, size: iced::Size, at: Point) -> Option<usize> {
        (0..self.state.bands.len())
            .map(|i| (i, self.handle_position(size, i).distance(at)))
            .filter(|(_, d)| *d <= HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

impl canvas::Program<Message> for EqPanel<'_> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let at = cursor.position_in(bounds)?;
                let hit = self.hit_test(bounds.size(), at)?;
                let now = Instant::now();
                let doubled = state
                    .last_click
                    .is_some_and(|(t, i)| i == hit && now.duration_since(t) < DOUBLE_CLICK);
                state.last_click = Some((now, hit));
                if doubled {
                    // Double-click: toggle the band.
                    state.drag = None;
                    let mut band = self.state.bands[hit];
                    band.enabled = !band.enabled;
                    return Some(
                        canvas::Action::publish(self.band_msg(hit, band, true)).and_capture(),
                    );
                }
                state.drag = Some(hit);
                state.press_at = Some(at);
                state.anchor = Some(self.handle_position(bounds.size(), hit));
                state.last = Some(at);
                state.moved = false;
                Some(canvas::Action::publish(self.select_msg(hit)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let band_index = *state.drag.as_ref()?;
                // Relative to the panel, and valid outside it: the drag follows the
                // pointer past the border and the anchor clamps below.
                let raw = cursor.position_from(bounds.position())?;

                // Still a selection until the pointer clears the slop radius.
                if !state.moved && state.press_at.is_some_and(|p| p.distance(raw) < DRAG_SLOP) {
                    return None;
                }
                state.moved = true;

                let mut band = self.state.bands[band_index];
                // The band moves by pointer travel, which preserves the grab point.
                let mut shift = raw - state.last.unwrap_or(raw);
                if state.modifiers.shift() {
                    shift *= FINE_DRAG;
                }
                if state.modifiers.control() {
                    // Lock to gain: frequency holds.
                    shift.x = 0.0;
                }
                if !band.kind.has_gain() {
                    shift.y = 0.0;
                }

                // Clamped each step, so the anchor stays inside the panel.
                let anchor = state.anchor.unwrap_or(raw) + shift;
                let anchor = Point::new(
                    anchor.x.clamp(0.0, bounds.width),
                    anchor.y.clamp(0.0, bounds.height),
                );
                state.anchor = Some(anchor);
                state.last = Some(raw);

                band.freq = freq_of_x(bounds.width, anchor.x);
                if band.kind.has_gain() {
                    band.gain_db = gain_of_y(bounds.height, anchor.y);
                }
                Some(canvas::Action::publish(self.band_msg(band_index, band, false)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take()?;
                state.press_at = None;
                state.anchor = None;
                state.last = None;
                // Only a real drag is persisted; a bare click has nothing to write.
                if !std::mem::take(&mut state.moved) {
                    return None;
                }
                match self.target {
                    // Drag release persists the global EQ; slot values are
                    // already live in the chain shadow.
                    EqTarget::Global => {
                        Some(canvas::Action::publish(Message::EqCommit).and_capture())
                    }
                    EqTarget::Slot(_) => None,
                }
            }
            canvas::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                // Mouse events carry no modifier state, so latch it here. Not captured:
                // Shift and Ctrl belong to the rest of the app too.
                state.modifiers = *m;
                None
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let at = cursor.position_in(bounds)?;
                let target = state.drag.or_else(|| self.hit_test(bounds.size(), at))?;
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                let mut band = self.state.bands[target];
                band.q = (band.q * Q_STEP.powf(lines)).clamp(Q_MIN, Q_MAX);
                Some(canvas::Action::publish(self.band_msg(target, band, true)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            let (w, h) = (frame.width(), frame.height());
            let thin = |color: Color, width: f32| canvas::Stroke {
                style: canvas::Style::Solid(color),
                width,
                ..canvas::Stroke::default()
            };
            let label = |content: String, position: Point, color: Color| canvas::Text {
                content,
                position,
                color,
                size: Pixels(10.0),
                font: Font::MONOSPACE,
                align_x: TextAlign::Center,
                ..canvas::Text::default()
            };

            // --- grid ---
            for freq in [
                30.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0,
            ] {
                let x = x_of_freq(w, freq);
                frame.stroke(
                    &canvas::Path::line(Point::new(x, 0.0), Point::new(x, h)),
                    thin(Color { a: 0.35, ..TRACK }, 1.0),
                );
            }
            for (freq, name) in [(100.0, "100"), (1_000.0, "1k"), (10_000.0, "10k")] {
                frame.fill_text(label(
                    name.to_string(),
                    Point::new(x_of_freq(w, freq), h - 14.0),
                    TEXT_DIM,
                ));
            }
            for db in [-12.0f32, -6.0, 6.0, 12.0] {
                let y = y_of_gain(h, db);
                frame.stroke(
                    &canvas::Path::line(Point::new(0.0, y), Point::new(w, y)),
                    thin(Color { a: 0.35, ..TRACK }, 1.0),
                );
                frame.fill_text(label(
                    format!("{db:+.0}"),
                    Point::new(14.0, y - 5.0),
                    TEXT_DIM,
                ));
            }
            let zero = y_of_gain(h, 0.0);
            frame.stroke(
                &canvas::Path::line(Point::new(0.0, zero), Point::new(w, zero)),
                thin(TRACK, 1.0),
            );

            // --- live output spectrum (filled) ---
            if self.spectrum.len() > 1 {
                let bins = self.spectrum.len();
                let path = canvas::Path::new(|b| {
                    b.move_to(Point::new(0.0, h));
                    for (i, &db) in self.spectrum.iter().enumerate() {
                        let x = w * (i as f32 + 0.5) / bins as f32;
                        b.line_to(Point::new(x, y_of_spectrum(h, db)));
                    }
                    b.line_to(Point::new(w, h));
                    b.close();
                });
                frame.fill(
                    &path,
                    Color {
                        a: 0.18,
                        ..METER_OK
                    },
                );
                let line = canvas::Path::new(|b| {
                    for (i, &db) in self.spectrum.iter().enumerate() {
                        let p =
                            Point::new(w * (i as f32 + 0.5) / bins as f32, y_of_spectrum(h, db));
                        if i == 0 {
                            b.move_to(p);
                        } else {
                            b.line_to(p);
                        }
                    }
                });
                frame.stroke(&line, thin(Color { a: 0.6, ..METER_OK }, 1.0));
                if let Some(tag) = self.spectrum_tag {
                    frame.fill_text(label(
                        tag.to_string(),
                        Point::new(w - 18.0, 6.0),
                        Color { a: 0.8, ..METER_OK },
                    ));
                }
            }

            // --- EQ response curve (the setting) ---
            let curve_color = if self.state.enabled { ACCENT } else { TEXT_DIM };
            let curve = canvas::Path::new(|b| {
                for i in 0..CURVE_POINTS {
                    let x = w * i as f32 / (CURVE_POINTS - 1) as f32;
                    let freq = freq_of_x(w, x);
                    let db = lh_dsp::eq::global::response_db(&self.state, self.sample_rate, freq);
                    let p = Point::new(x, y_of_gain(h, db.clamp(-GAIN_DB_MAX, GAIN_DB_MAX)));
                    if i == 0 {
                        b.move_to(p);
                    } else {
                        b.line_to(p);
                    }
                }
            });
            frame.stroke(&curve, thin(curve_color, 2.0));

            // --- band handles ---
            for (i, band) in self.state.bands.iter().enumerate() {
                let at = self.handle_position(frame.size(), i);
                let selected = i == self.selected;
                let dragging = state.drag == Some(i);
                let color = if selected || dragging {
                    ACCENT
                } else if band.enabled {
                    TEXT_BRIGHT
                } else {
                    TEXT_DIM
                };
                if band.enabled {
                    frame.fill(&canvas::Path::circle(at, HANDLE_RADIUS), color);
                    frame.fill(&canvas::Path::circle(at, HANDLE_RADIUS - 2.5), PANEL_HI);
                    frame.fill(&canvas::Path::circle(at, 2.5), color);
                } else {
                    frame.stroke(
                        &canvas::Path::circle(at, HANDLE_RADIUS - 1.0),
                        thin(color, 1.5),
                    );
                }
                frame.fill_text(label(
                    format!("{}", i + 1),
                    Point::new(at.x, at.y - HANDLE_RADIUS - 13.0),
                    color,
                ));
            }
        });
        let _ = cursor;
        vec![geometry]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if let Some(at) = cursor.position_in(bounds)
            && self.hit_test(bounds.size(), at).is_some()
        {
            return mouse::Interaction::Grab;
        }
        mouse::Interaction::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::canvas::Program;

    /// Drag is pure logic reachable without a window, and it shipped with no coverage.
    /// If these pass while the GUI stays inert, the fault is event delivery or mounting
    /// rather than the handler.
    /// Drag one band by an explicit delta and report what the published edit carried.
    /// The delta is a parameter because the outermost handles sit near the panel edges,
    /// and a drag that leaves the canvas is correctly ignored — the test has to aim
    /// inward, not off the side.
    fn drag_band(index: usize, dx: f32, dy: f32) -> Band {
        let cache = canvas::Cache::new();
        let spectrum: Vec<f32> = Vec::new();
        let panel = EqPanel {
            state: GlobalEqState::default(),
            target: EqTarget::Global,
            selected: index,
            spectrum: &spectrum,
            spectrum_tag: None,
            sample_rate: 48_000.0,
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 400.0));
        let mut state = State::default();

        let handle = panel.handle_position(bounds.size(), index);
        let press = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let action = panel.update(&mut state, &press, bounds, mouse::Cursor::Available(handle));
        assert!(action.is_some(), "pressing a handle must be handled");
        assert_eq!(state.drag, Some(index), "press must arm the drag");

        let to = Point::new(handle.x + dx, handle.y + dy);
        let moved = canvas::Event::Mouse(mouse::Event::CursorMoved { position: to });
        let action = panel
            .update(&mut state, &moved, bounds, mouse::Cursor::Available(to))
            .expect("a move while dragging must publish an edit");

        let (published, _, _) = action.into_inner();
        let Some(Message::EqBand {
            index: got, band, ..
        }) = published
        else {
            panic!("expected a published Message::EqBand");
        };
        assert_eq!(got, index);
        band
    }

    /// A bell band moves in both axes. This is the interaction the panel advertises,
    /// and it had no coverage before.
    #[test]
    fn dragging_a_bell_changes_frequency_and_gain() {
        let before = GlobalEqState::default().bands[3];
        let after = drag_band(3, 120.0, -60.0);
        assert!(
            after.freq > before.freq,
            "drag right must raise frequency: {} -> {}",
            before.freq,
            after.freq
        );
        assert!(
            after.gain_db > before.gain_db,
            "drag up must raise gain: {} -> {}",
            before.gain_db,
            after.gain_db
        );
    }

    /// The outermost two handles are cut filters, which have no gain to drag. Vertical
    /// motion on them is *correctly* inert — worth pinning, because those two sit at the
    /// far left and far right of the panel where they are the most obvious things to
    /// grab first, and an inert drag there reads as "the EQ is broken".
    #[test]
    fn dragging_a_cut_band_moves_frequency_only() {
        // Aim inward from each edge: band 0 sits at 30 Hz on the left, band 7 at
        // 12 kHz on the right.
        for (index, dx) in [(0usize, 120.0f32), (7usize, -120.0f32)] {
            let before = GlobalEqState::default().bands[index];
            assert!(
                !before.kind.has_gain(),
                "band {index} should be a cut filter"
            );
            let after = drag_band(index, dx, -60.0);
            if dx > 0.0 {
                assert!(
                    after.freq > before.freq,
                    "band {index}: dragging right must still raise frequency"
                );
            } else {
                assert!(
                    after.freq < before.freq,
                    "band {index}: dragging left must still lower frequency"
                );
            }
            assert_eq!(
                after.gain_db, before.gain_db,
                "band {index}: a cut filter has no gain to move"
            );
        }
    }

    /// Every band ships disabled, so a fresh EQ is transparent. Pinned because it is
    /// the premise the auto-enable below exists to rescue: without it, a drag on an
    /// untouched panel edits a bypassed band and changes no sound.
    #[test]
    fn every_band_starts_disabled() {
        let state = GlobalEqState::default();
        assert!(state.enabled, "the EQ section itself is on");
        assert!(
            state.bands.iter().all(|b| !b.enabled),
            "a default EQ is transparent: every band starts disabled"
        );
    }

    /// Dragging edits a band but never switches it on: enabling stays an explicit
    /// double-click, so the panel loads flat unless it was deliberately set.
    #[test]
    fn dragging_does_not_enable_a_band() {
        for index in [0usize, 3, 7] {
            assert!(
                !GlobalEqState::default().bands[index].enabled,
                "precondition: band {index} starts disabled"
            );
            let dx = if index == 7 { -120.0 } else { 120.0 };
            let after = drag_band(index, dx, -60.0);
            assert!(!after.enabled, "band {index}: a drag must not switch it on");
        }
    }

    /// A click — even a shaky one — selects without editing. The EQ must load flat
    /// unless it was deliberately set, and auto-enable would otherwise let a pixel of
    /// hand-shake switch a band on and persist it.
    #[test]
    fn a_click_with_hand_shake_neither_edits_nor_commits() {
        let cache = canvas::Cache::new();
        let spectrum: Vec<f32> = Vec::new();
        let panel = EqPanel {
            state: GlobalEqState::default(),
            target: EqTarget::Global,
            selected: 3,
            spectrum: &spectrum,
            spectrum_tag: None,
            sample_rate: 48_000.0,
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 400.0));
        let mut state = State::default();
        let handle = panel.handle_position(bounds.size(), 3);

        let press = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        panel.update(&mut state, &press, bounds, mouse::Cursor::Available(handle));

        // Two pixels of shake, inside DRAG_SLOP.
        let jitter = Point::new(handle.x + 1.5, handle.y + 1.0);
        let moved = canvas::Event::Mouse(mouse::Event::CursorMoved { position: jitter });
        assert!(
            panel
                .update(&mut state, &moved, bounds, mouse::Cursor::Available(jitter))
                .is_none(),
            "a click within the slop radius must not publish an edit"
        );
        assert!(!state.moved, "slop-sized motion is not a drag");

        // Release must not commit either: nothing changed, so nothing to persist.
        let release = canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        assert!(
            panel
                .update(
                    &mut state,
                    &release,
                    bounds,
                    mouse::Cursor::Available(jitter)
                )
                .is_none(),
            "releasing a click must not commit the EQ to disk"
        );
    }

    /// Grabbing a handle off-centre must not teleport it onto the pointer. The hit
    /// radius is twice the drawn handle, so a legitimate grab can start 14 px away —
    /// without the offset the band jumped that far the moment the drag began, which
    /// is most of what made the panel feel unpredictable.
    #[test]
    fn grabbing_off_centre_moves_by_travel_not_to_the_pointer() {
        let cache = canvas::Cache::new();
        let spectrum: Vec<f32> = Vec::new();
        let panel = EqPanel {
            state: GlobalEqState::default(),
            target: EqTarget::Global,
            selected: 3,
            spectrum: &spectrum,
            spectrum_tag: None,
            sample_rate: 48_000.0,
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 400.0));
        let mut state = State::default();
        let handle = panel.handle_position(bounds.size(), 3);

        // Grab 10 px right of centre — inside HIT_RADIUS, so a real grab.
        let grab = Point::new(handle.x + 10.0, handle.y);
        let press = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        panel.update(&mut state, &press, bounds, mouse::Cursor::Available(grab));

        // Travel 50 px further right.
        let to = Point::new(grab.x + 50.0, grab.y);
        let moved = canvas::Event::Mouse(mouse::Event::CursorMoved { position: to });
        let (published, _, _) = panel
            .update(&mut state, &moved, bounds, mouse::Cursor::Available(to))
            .expect("a real drag publishes")
            .into_inner();
        let Some(Message::EqBand { band, .. }) = published else {
            panic!("expected Message::EqBand");
        };

        // The band should sit 50 px from where it started, not 60 px at the pointer.
        let want = freq_of_x(bounds.width, handle.x + 50.0);
        let pointer = freq_of_x(bounds.width, to.x);
        assert!(
            (band.freq - want).abs() < 1.0,
            "band should track travel: expected ~{want:.1} Hz, got {:.1} Hz",
            band.freq
        );
        assert!(
            (band.freq - pointer).abs() > 1.0,
            "band must not snap onto the pointer ({pointer:.1} Hz)"
        );
    }

    /// A drag that leaves the canvas clamps at the edge and keeps tracking.
    #[test]
    fn dragging_past_the_edge_clamps_instead_of_freezing() {
        let cache = canvas::Cache::new();
        let mut state = State::default();
        let panel = EqPanel {
            state: GlobalEqState::default(),
            target: EqTarget::Global,
            selected: 3,
            spectrum: &[],
            spectrum_tag: None,
            sample_rate: 48_000.0,
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 400.0));
        let handle = panel.handle_position(bounds.size(), 3);

        let press = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        panel.update(&mut state, &press, bounds, mouse::Cursor::Available(handle));

        // Way past the right edge and below the bottom.
        let far = Point::new(bounds.width + 500.0, bounds.height + 500.0);
        let moved = canvas::Event::Mouse(mouse::Event::CursorMoved { position: far });
        let (published, _, _) = panel
            .update(&mut state, &moved, bounds, mouse::Cursor::Available(far))
            .expect("a drag outside the canvas must still track")
            .into_inner();
        let Some(Message::EqBand { band, .. }) = published else {
            panic!("expected Message::EqBand");
        };
        assert!(
            (band.freq - FREQ_MAX).abs() < 1.0,
            "frequency should clamp to the top of the axis, got {:.1} Hz",
            band.freq
        );
        assert!(
            band.gain_db <= -GAIN_DB_MAX + 0.01,
            "gain should clamp to the bottom of the axis, got {:+.2} dB",
            band.gain_db
        );
    }

    /// Shift scales travel, for setting a value precisely.
    #[test]
    fn shift_makes_the_drag_finer() {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 400.0));

        let coarse = drag_band(3, 100.0, 0.0).freq;

        let cache = canvas::Cache::new();
        let panel = EqPanel {
            state: GlobalEqState::default(),
            target: EqTarget::Global,
            selected: 3,
            spectrum: &[],
            spectrum_tag: None,
            sample_rate: 48_000.0,
            cache: &cache,
        };
        let mut state = State::default();
        let mods = canvas::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(
            iced::keyboard::Modifiers::SHIFT,
        ));
        panel.update(&mut state, &mods, bounds, mouse::Cursor::Unavailable);

        let handle = panel.handle_position(bounds.size(), 3);
        let press = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        panel.update(&mut state, &press, bounds, mouse::Cursor::Available(handle));
        let to = Point::new(handle.x + 100.0, handle.y);
        let moved = canvas::Event::Mouse(mouse::Event::CursorMoved { position: to });
        let (published, _, _) = panel
            .update(&mut state, &moved, bounds, mouse::Cursor::Available(to))
            .expect("shift-drag still publishes")
            .into_inner();
        let Some(Message::EqBand { band, .. }) = published else {
            panic!("expected Message::EqBand");
        };

        let base = GlobalEqState::default().bands[3].freq;
        assert!(
            band.freq > base && band.freq < coarse,
            "shift-drag should move less than the same travel unshifted: \
             base {base:.0} Hz, shift {:.0} Hz, coarse {coarse:.0} Hz",
            band.freq
        );
    }
}
