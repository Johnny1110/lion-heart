//! The global EQ editor (PRD 003): a log-frequency canvas with two overlaid
//! curves — the EQ's composite response (setting) and the live output
//! spectrum (playing) — plus draggable band handles.
//!
//! Interactions: drag a handle for freq/gain (cut bands: freq only), wheel
//! over it for Q, double-click to enable/disable. The detail strip below
//! the canvas (in `gui::mod`) covers type changes and numeric readouts.

use std::time::{Duration, Instant};

use iced::widget::canvas;
use iced::widget::text::Alignment as TextAlign;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Theme, mouse};
use lh_core::global_eq::{FREQ_MAX, FREQ_MIN, GAIN_DB_MAX, GlobalEqState, Q_MAX, Q_MIN};

use super::Message;
use super::spectrum::DB_FLOOR;
use super::theme::{ACCENT, METER_OK, PANEL_HI, TEXT_BRIGHT, TEXT_DIM, TRACK};

const HIT_RADIUS: f32 = 14.0;
const HANDLE_RADIUS: f32 = 7.0;
const CURVE_POINTS: usize = 160;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Wheel sensitivity: Q multiplier per scroll line.
const Q_STEP: f32 = 1.12;

pub struct EqPanel<'a> {
    pub state: &'a GlobalEqState,
    pub selected: usize,
    /// Display bins from the spectrum analyzer (dBFS).
    pub spectrum: &'a [f32],
    pub sample_rate: f32,
    pub cache: &'a canvas::Cache,
}

#[derive(Default)]
pub struct State {
    drag: Option<usize>,
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
                        canvas::Action::publish(Message::EqBand {
                            index: hit,
                            band,
                            commit: true,
                        })
                        .and_capture(),
                    );
                }
                state.drag = Some(hit);
                Some(canvas::Action::publish(Message::EqSelect(hit)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let band_index = *state.drag.as_ref()?;
                let at = cursor.position_in(bounds)?;
                let mut band = self.state.bands[band_index];
                band.freq = freq_of_x(bounds.width, at.x);
                if band.kind.has_gain() {
                    band.gain_db = gain_of_y(bounds.height, at.y);
                }
                Some(
                    canvas::Action::publish(Message::EqBand {
                        index: band_index,
                        band,
                        commit: false,
                    })
                    .and_capture(),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take()?;
                Some(canvas::Action::publish(Message::EqCommit).and_capture())
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
                Some(
                    canvas::Action::publish(Message::EqBand {
                        index: target,
                        band,
                        commit: true,
                    })
                    .and_capture(),
                )
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
            }

            // --- EQ response curve (the setting) ---
            let curve_color = if self.state.enabled { ACCENT } else { TEXT_DIM };
            let curve = canvas::Path::new(|b| {
                for i in 0..CURVE_POINTS {
                    let x = w * i as f32 / (CURVE_POINTS - 1) as f32;
                    let freq = freq_of_x(w, x);
                    let db = lh_dsp::param_eq::response_db(self.state, self.sample_rate, freq);
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
