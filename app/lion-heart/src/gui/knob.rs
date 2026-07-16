//! The rotary knob: a 270° arc, dragged vertically, publishing normalized
//! values keyed by (slot, param). Name above, value readout below, all drawn
//! in one cached canvas that only re-renders when the value changes.

use iced::widget::canvas;
use iced::widget::text::Alignment as TextAlign;
use iced::{Color, Font, Pixels, Point, Radians, Rectangle, Renderer, Theme, mouse};

use super::Message;
use super::theme::{ACCENT, TEXT_BRIGHT, TEXT_DIM, TRACK};

pub const WIDTH: f32 = 96.0;
pub const HEIGHT: f32 = 118.0;

/// Full drag travel in pixels for min → max.
const DRAG_RANGE_PX: f32 = 160.0;
const SWEEP_START: f32 = 0.75 * std::f32::consts::PI; // 135°: lower left
const SWEEP: f32 = 1.5 * std::f32::consts::PI; // 270° clockwise

pub struct Knob<'a> {
    pub slot: String,
    pub param: String,
    pub name: &'static str,
    pub value: String,
    pub norm: f32,
    pub cache: &'a canvas::Cache,
}

#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
}

struct Drag {
    start_y: f32,
    start_norm: f32,
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
                state.drag = Some(Drag {
                    start_y: cursor.position()?.y,
                    start_norm: self.norm,
                });
                Some(canvas::Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let drag = state.drag.as_ref()?;
                let norm =
                    (drag.start_norm + (drag.start_y - position.y) / DRAG_RANGE_PX).clamp(0.0, 1.0);
                Some(
                    canvas::Action::publish(Message::Knob {
                        slot: self.slot.clone(),
                        param: self.param.clone(),
                        norm,
                    })
                    .and_capture(),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.drag.take()?;
                Some(canvas::Action::capture())
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
            let center = Point::new(frame.width() / 2.0, frame.height() / 2.0 + 2.0);
            let radius = 30.0;
            let angle = SWEEP_START + self.norm * SWEEP;

            let arc = |from: f32, to: f32| {
                canvas::Path::new(|b| {
                    b.arc(canvas::path::Arc {
                        center,
                        radius,
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

            frame.stroke(&arc(SWEEP_START, SWEEP_START + SWEEP), stroke(TRACK, 5.0));
            if self.norm > 0.001 {
                frame.stroke(&arc(SWEEP_START, angle), stroke(ACCENT, 5.0));
            }
            let tip = |r: f32| Point::new(center.x + r * angle.cos(), center.y + r * angle.sin());
            frame.stroke(
                &canvas::Path::line(tip(radius * 0.3), tip(radius * 0.8)),
                stroke(TEXT_BRIGHT, 3.0),
            );

            frame.fill_text(label(self.name.to_uppercase(), 2.0, TEXT_DIM, 12.0));
            frame.fill_text(label(
                self.value.clone(),
                center.y + radius + 12.0,
                TEXT_BRIGHT,
                13.0,
            ));
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
