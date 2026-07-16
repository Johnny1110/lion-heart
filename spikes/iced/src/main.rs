//! M4 GUI spike — iced 0.14.
//!
//! One screen: a custom-drawn rotary knob bound to `drive.drive` on the real
//! engine chain, and realtime input/output peak meters fed from `Telemetry`,
//! redrawn every frame via the `window::frames()` subscription. The point is
//! to measure custom-widget cost and render behavior, not to look final.

use std::time::Instant;

use iced::widget::canvas::{self, Canvas};
use iced::widget::text::Alignment as TextAlign;
use iced::widget::{center_x, column, container, text};
use iced::{
    Color, Element, Font, Length, Pixels, Point, Radians, Rectangle, Renderer, Size, Subscription,
    Theme, mouse, window,
};
use spike_common::{METER_FLOOR_DB, MeterBallistics, SpikeEngine};

const BG: Color = Color::from_rgb(0.09, 0.09, 0.11);
const TRACK: Color = Color::from_rgb(0.23, 0.23, 0.27);
const ACCENT: Color = Color::from_rgb(1.0, 0.55, 0.24);
const TEXT_DIM: Color = Color::from_rgb(0.55, 0.55, 0.60);
const TEXT_BRIGHT: Color = Color::from_rgb(0.92, 0.92, 0.94);
const METER_OK: Color = Color::from_rgb(0.30, 0.78, 0.42);
const METER_HOT: Color = Color::from_rgb(0.92, 0.26, 0.21);
const HOT_DB: f32 = -6.0;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .subscription(App::subscription)
        .theme(App::theme)
        .antialiasing(true)
        .window_size(Size::new(420.0, 560.0))
        .title("Lion-Heart — iced spike")
        .run()
}

struct App {
    engine: SpikeEngine,
    ballistics: MeterBallistics,
    knob_cache: canvas::Cache,
    meter_cache: canvas::Cache,
    last_frame: Option<Instant>,
    frame_secs: f32,
}

#[derive(Debug, Clone)]
enum Message {
    Frame(Instant),
    DriveChanged(f32),
}

impl App {
    fn new() -> Self {
        Self {
            engine: SpikeEngine::start(),
            ballistics: MeterBallistics::new(),
            knob_cache: canvas::Cache::new(),
            meter_cache: canvas::Cache::new(),
            last_frame: None,
            frame_secs: 1.0 / 60.0,
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Frame(now) => {
                if let Some(last) = self.last_frame {
                    let dt = (now - last).as_secs_f32();
                    self.frame_secs = 0.9 * self.frame_secs + 0.1 * dt;
                }
                self.last_frame = Some(now);
                self.ballistics.tick(self.engine.peaks());
                self.meter_cache.clear();
            }
            Message::DriveChanged(norm) => {
                self.engine.set_drive_norm(norm);
                self.knob_cache.clear();
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        window::frames().map(Message::Frame)
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn view(&self) -> Element<'_, Message> {
        let knob = Canvas::new(Knob {
            norm: self.engine.drive_norm(),
            label: self.engine.drive_display(),
            cache: &self.knob_cache,
        })
        .width(200)
        .height(220);

        let meters = Canvas::new(Meters {
            norms: self.ballistics.norms(),
            cache: &self.meter_cache,
        })
        .width(Length::Fill)
        .height(130);

        let fps = 1.0 / self.frame_secs.max(1e-4);
        let content = column![
            text("DRIVE").size(16).color(TEXT_DIM),
            center_x(knob),
            meters,
            text(format!("{fps:.0} fps — drag the knob, watch the meters"))
                .size(13)
                .color(TEXT_DIM),
        ]
        .spacing(18)
        .padding(24);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style::default().background(BG))
            .into()
    }
}

// --- custom rotary knob -----------------------------------------------------

/// 270° rotary knob: drag vertically to change, published as normalized 0..1.
struct Knob<'a> {
    norm: f32,
    label: String,
    cache: &'a canvas::Cache,
}

#[derive(Default)]
struct KnobState {
    drag: Option<Drag>,
}

struct Drag {
    start_y: f32,
    start_norm: f32,
}

/// Full drag travel in pixels for min → max.
const DRAG_RANGE_PX: f32 = 160.0;
const SWEEP_START: f32 = 0.75 * std::f32::consts::PI; // 135°: lower left
const SWEEP: f32 = 1.5 * std::f32::consts::PI; // 270° clockwise

impl canvas::Program<Message> for Knob<'_> {
    type State = KnobState;

    fn update(
        &self,
        state: &mut KnobState,
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
                Some(canvas::Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let drag = state.drag.as_ref()?;
                let norm =
                    (drag.start_norm + (drag.start_y - position.y) / DRAG_RANGE_PX).clamp(0.0, 1.0);
                Some(canvas::Action::publish(Message::DriveChanged(norm)).and_capture())
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
        _state: &KnobState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            let center = Point::new(frame.width() / 2.0, frame.height() / 2.0 - 12.0);
            let radius = 62.0;
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

            // Track, then the value arc on top of it.
            frame.stroke(&arc(SWEEP_START, SWEEP_START + SWEEP), stroke(TRACK, 7.0));
            if self.norm > 0.001 {
                frame.stroke(&arc(SWEEP_START, angle), stroke(ACCENT, 7.0));
            }

            // Pointer line.
            let dir = |r: f32| Point::new(center.x + r * angle.cos(), center.y + r * angle.sin());
            frame.stroke(
                &canvas::Path::line(dir(radius * 0.35), dir(radius * 0.82)),
                stroke(TEXT_BRIGHT, 4.0),
            );

            // Value readout under the knob.
            frame.fill_text(canvas::Text {
                content: self.label.clone(),
                position: Point::new(center.x, center.y + radius + 26.0),
                color: TEXT_BRIGHT,
                size: Pixels(18.0),
                font: Font::MONOSPACE,
                align_x: TextAlign::Center,
                ..canvas::Text::default()
            });
        });
        vec![geometry]
    }

    fn mouse_interaction(
        &self,
        state: &KnobState,
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

// --- peak meters --------------------------------------------------------------

/// Horizontal IN/OUT peak bars over a -60..0 dBFS scale, hot zone above -6 dB.
struct Meters<'a> {
    norms: (f32, f32),
    cache: &'a canvas::Cache,
}

impl canvas::Program<Message> for Meters<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            let label_w = 40.0;
            let bar_w = frame.width() - label_w - 8.0;
            let bar_h = 22.0;
            let hot_norm = (HOT_DB - METER_FLOOR_DB) / -METER_FLOOR_DB;

            let mut draw_bar = |y: f32, label: &str, norm: f32| {
                frame.fill_text(canvas::Text {
                    content: label.into(),
                    position: Point::new(0.0, y + bar_h / 2.0 - 8.0),
                    color: TEXT_DIM,
                    size: Pixels(14.0),
                    font: Font::MONOSPACE,
                    ..canvas::Text::default()
                });
                frame.fill_rectangle(
                    Point::new(label_w, y),
                    Size::new(bar_w, bar_h),
                    Color::from_rgb(0.14, 0.14, 0.17),
                );
                let ok = norm.min(hot_norm);
                if ok > 0.0 {
                    frame.fill_rectangle(
                        Point::new(label_w, y),
                        Size::new(bar_w * ok, bar_h),
                        METER_OK,
                    );
                }
                if norm > hot_norm {
                    frame.fill_rectangle(
                        Point::new(label_w + bar_w * hot_norm, y),
                        Size::new(bar_w * (norm - hot_norm), bar_h),
                        METER_HOT,
                    );
                }
            };
            draw_bar(8.0, "IN", self.norms.0);
            draw_bar(8.0 + bar_h + 14.0, "OUT", self.norms.1);

            // dB scale ticks.
            let scale_y = 8.0 + 2.0 * bar_h + 14.0 + 8.0;
            for db in [-60.0f32, -40.0, -20.0, -6.0, 0.0] {
                let norm = (db - METER_FLOOR_DB) / -METER_FLOOR_DB;
                let x = label_w + bar_w * norm;
                frame.fill_rectangle(Point::new(x - 0.5, scale_y), Size::new(1.0, 6.0), TEXT_DIM);
                frame.fill_text(canvas::Text {
                    content: format!("{db:.0}"),
                    position: Point::new(x, scale_y + 8.0),
                    color: TEXT_DIM,
                    size: Pixels(11.0),
                    font: Font::MONOSPACE,
                    align_x: TextAlign::Center,
                    ..canvas::Text::default()
                });
            }
        });
        vec![geometry]
    }
}
