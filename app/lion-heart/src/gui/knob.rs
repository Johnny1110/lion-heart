//! The rotary knob: a ticked 270° dial whose arc glows in the owning
//! pedal's identity color. Dragged vertically; the wheel nudges the value;
//! double-click snaps back to the pedal's default. Name above, a recessed
//! value readout below, all drawn in one cached canvas that only
//! re-renders when the value changes.

use std::time::{Duration, Instant};

use iced::widget::canvas;
use iced::widget::text::Alignment as TextAlign;
use iced::{Color, Font, Pixels, Point, Radians, Rectangle, Renderer, Size, Theme, mouse};

use super::Message;
use super::theme::{INSET, PANEL_HI, TEXT_BRIGHT, TEXT_DIM, TRACK, dim};

pub const WIDTH: f32 = 96.0;
pub const HEIGHT: f32 = 128.0;

/// Full drag travel in pixels for min → max.
const DRAG_RANGE_PX: f32 = 160.0;
/// Wheel nudge per scroll line (2% of travel).
const WHEEL_STEP: f32 = 0.02;
const DOUBLE_CLICK: Duration = Duration::from_millis(350);
const SWEEP_START: f32 = 0.75 * std::f32::consts::PI; // 135°: lower left
const SWEEP: f32 = 1.5 * std::f32::consts::PI; // 270° clockwise

pub struct Knob<'a> {
    pub slot: String,
    pub param: String,
    pub name: &'static str,
    pub value: String,
    pub norm: f32,
    /// The faceplate default — double-click returns here.
    pub default_norm: f32,
    /// The owning pedal's identity color (arc + glow).
    pub accent: Color,
    pub cache: &'a canvas::Cache,
}

#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
    last_press: Option<Instant>,
}

struct Drag {
    start_y: f32,
    start_norm: f32,
}

impl Knob<'_> {
    fn publish(&self, norm: f32) -> canvas::Action<Message> {
        canvas::Action::publish(Message::Knob {
            slot: self.slot.clone(),
            param: self.param.clone(),
            norm: norm.clamp(0.0, 1.0),
        })
        .and_capture()
    }
}

impl canvas::Program<Message> for Knob<'_> {
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
                cursor.position_in(bounds)?;
                let now = Instant::now();
                let doubled = state
                    .last_press
                    .is_some_and(|at| now.duration_since(at) < DOUBLE_CLICK);
                state.last_press = Some(now);
                if doubled {
                    // Double-click: back to the faceplate default.
                    state.drag = None;
                    return Some(self.publish(self.default_norm));
                }
                state.drag = Some(Drag {
                    start_y: cursor.position()?.y,
                    start_norm: self.norm,
                });
                Some(canvas::Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let drag = state.drag.as_ref()?;
                let norm = drag.start_norm + (drag.start_y - position.y) / DRAG_RANGE_PX;
                Some(self.publish(norm))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take()?;
                Some(canvas::Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                cursor.position_in(bounds)?;
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                Some(self.publish(self.norm + lines * WHEEL_STEP))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            let center = Point::new(frame.width() / 2.0, 64.0);
            let radius = 26.0;
            let angle = SWEEP_START + self.norm * SWEEP;

            let arc = |r: f32, from: f32, to: f32| {
                canvas::Path::new(|b| {
                    b.arc(canvas::path::Arc {
                        center,
                        radius: r,
                        start_angle: Radians(from),
                        end_angle: Radians(to),
                    });
                })
            };
            let stroke = |color: Color, width: f32| canvas::Stroke {
                style: canvas::Style::Solid(color),
                width,
                line_cap: canvas::LineCap::Round,
                ..canvas::Stroke::default()
            };
            let label = |content: String, y: f32, color: Color, size: f32| canvas::Text {
                content,
                position: Point::new(center.x, y),
                color,
                size: Pixels(size),
                font: Font::MONOSPACE,
                align_x: TextAlign::Center,
                ..canvas::Text::default()
            };

            // Tick ring: 0..10, majors at the ends and noon.
            for i in 0..=10 {
                let a = SWEEP_START + i as f32 / 10.0 * SWEEP;
                let major = i == 0 || i == 5 || i == 10;
                let (r0, r1) = if major { (31.0, 36.0) } else { (32.0, 35.0) };
                let at = |r: f32| Point::new(center.x + r * a.cos(), center.y + r * a.sin());
                frame.stroke(
                    &canvas::Path::line(at(r0), at(r1)),
                    stroke(if major { TEXT_DIM } else { dim(TRACK, 0.8) }, 1.5),
                );
            }

            // Track, then the value arc over a soft glow of the same color.
            frame.stroke(
                &arc(radius, SWEEP_START, SWEEP_START + SWEEP),
                stroke(dim(TRACK, 0.9), 4.0),
            );
            if self.norm > 0.001 {
                frame.stroke(
                    &arc(radius, SWEEP_START, angle),
                    stroke(dim(self.accent, 0.28), 9.0),
                );
                frame.stroke(&arc(radius, SWEEP_START, angle), stroke(self.accent, 4.0));
            }

            // Cap and pointer.
            frame.fill(&canvas::Path::circle(center, 16.0), PANEL_HI);
            frame.stroke(
                &canvas::Path::circle(center, 16.0),
                stroke(dim(TRACK, 0.9), 1.0),
            );
            let tip = |r: f32| Point::new(center.x + r * angle.cos(), center.y + r * angle.sin());
            frame.stroke(
                &canvas::Path::line(tip(5.0), tip(14.0)),
                stroke(TEXT_BRIGHT, 2.5),
            );

            // Name above, recessed value readout below.
            frame.fill_text(label(self.name.to_uppercase(), 2.0, TEXT_DIM, 11.0));
            let pill = Size::new(80.0, 19.0);
            let pill_at = Point::new(center.x - pill.width / 2.0, 104.0);
            frame.fill(
                &canvas::Path::rounded_rectangle(pill_at, pill, 5.0.into()),
                INSET,
            );
            frame.fill_text(label(self.value.clone(), 107.0, TEXT_BRIGHT, 12.0));
        });
        vec![geometry]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}
