//! M4 GUI spike — vizia 0.4.
//!
//! Same screen as the iced spike: a custom-drawn rotary knob bound to
//! `drive.drive` on the real engine chain, and realtime peak meters fed from
//! `Telemetry`, polled by a 16 ms timer. vizia 0.4 replaced its old
//! Lens/Model system with global reactive signals; state below follows the
//! new idiom (`Signal` + `Handle::bind`), custom drawing goes through
//! skia-safe (`vg`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use spike_common::{METER_FLOOR_DB, MeterBallistics, SpikeEngine};
use vizia::prelude::*;
use vizia::vg;

const HOT_DB: f32 = -6.0;
/// Full drag travel in pixels for min → max (matches the iced spike).
const DRAG_RANGE_PX: f32 = 160.0;
const SWEEP_START_DEG: f32 = 135.0;
const SWEEP_DEG: f32 = 270.0;

fn accent() -> vg::Color {
    vg::Color::from_argb(255, 255, 140, 61)
}
fn track() -> vg::Color {
    vg::Color::from_argb(255, 59, 59, 69)
}
fn bright() -> vg::Color {
    vg::Color::from_argb(255, 235, 235, 240)
}

fn main() -> Result<(), ApplicationError> {
    Application::new(|cx| {
        let engine = Rc::new(RefCell::new(SpikeEngine::start()));

        let drive_norm = Signal::new(engine.borrow().drive_norm());
        let drive_label = Signal::new(engine.borrow().drive_display());
        let meter_in = Signal::new(0.0f32);
        let meter_out = Signal::new(0.0f32);
        let fps = Signal::new(60.0f32);

        // 60 fps meter poll — counterpart of iced's window::frames() subscription.
        let timer = cx.add_timer(Duration::from_millis(16), None, {
            let engine = Rc::clone(&engine);
            let ballistics = RefCell::new(MeterBallistics::new());
            let last = Cell::new(None::<Instant>);
            let frame_secs = Cell::new(1.0f32 / 60.0);
            move |_cx, action| {
                if let TimerAction::Tick(_) = action {
                    let now = Instant::now();
                    if let Some(prev) = last.get() {
                        let dt = (now - prev).as_secs_f32();
                        frame_secs.set(0.9 * frame_secs.get() + 0.1 * dt);
                        fps.set(1.0 / frame_secs.get().max(1e-4));
                    }
                    last.set(Some(now));
                    let mut b = ballistics.borrow_mut();
                    b.tick(engine.borrow().peaks());
                    let (i, o) = b.norms();
                    meter_in.set(i);
                    meter_out.set(o);
                }
            }
        });
        cx.start_timer(timer);

        VStack::new(cx, |cx| {
            Label::new(cx, "DRIVE")
                .color(Color::rgb(140, 140, 153))
                .font_size(16.0);

            KnobView::new(cx, drive_norm.read_only())
                .on_change({
                    let engine = Rc::clone(&engine);
                    move |_cx, norm| {
                        let mut e = engine.borrow_mut();
                        e.set_drive_norm(norm);
                        drive_norm.set(e.drive_norm());
                        drive_label.set(e.drive_display());
                    }
                })
                .size(Pixels(160.0));

            Label::new(cx, drive_label)
                .color(Color::rgb(235, 235, 240))
                .font_size(18.0);

            meter_row(cx, "IN", meter_in.read_only());
            meter_row(cx, "OUT", meter_out.read_only());

            Label::new(
                cx,
                fps.map(|f| format!("{f:.0} fps — drag the knob, watch the meters")),
            )
            .color(Color::rgb(140, 140, 153))
            .font_size(13.0);
        })
        .padding(Pixels(24.0))
        .gap(Pixels(18.0))
        .alignment(Alignment::TopCenter)
        .background_color(Color::rgb(23, 23, 28));
    })
    .title("Lion-Heart — vizia spike")
    .inner_size((420, 560))
    .run()
}

fn meter_row(cx: &mut Context, label: &'static str, value: ReadSignal<f32>) {
    HStack::new(cx, |cx| {
        Label::new(cx, label)
            .color(Color::rgb(140, 140, 153))
            .font_size(14.0)
            .width(Pixels(40.0));
        MeterView::new(cx, value)
            .height(Pixels(22.0))
            .width(Stretch(1.0));
    })
    .height(Auto)
    .alignment(Alignment::Center);
}

// --- custom rotary knob -----------------------------------------------------

type ChangeCallback = Box<dyn Fn(&mut EventContext, f32)>;

/// 270° rotary knob: drag vertically to change, reported via `on_change`
/// as normalized 0..1. Value flows back in through a signal binding.
pub struct KnobView {
    norm: f32,
    dragging: bool,
    prev_y: f32,
    on_change: Option<ChangeCallback>,
}

impl KnobView {
    pub fn new(cx: &mut Context, value: ReadSignal<f32>) -> Handle<'_, Self> {
        Self {
            norm: value.get(),
            dragging: false,
            prev_y: 0.0,
            on_change: None,
        }
        .build(cx, |_| {})
        .bind(value, move |handle| {
            let mut handle = handle.modify(|knob| knob.norm = value.get());
            handle.needs_redraw();
        })
    }
}

/// Modifier for [`KnobView`] handles. Unlike vizia's built-in views (which
/// write inherent impls on `Handle` inside vizia_core), app code is on the
/// wrong side of the orphan rule and needs an extension trait.
pub trait KnobHandle {
    fn on_change(self, callback: impl 'static + Fn(&mut EventContext, f32)) -> Self;
}

impl KnobHandle for Handle<'_, KnobView> {
    fn on_change(self, callback: impl 'static + Fn(&mut EventContext, f32)) -> Self {
        self.modify(|knob| knob.on_change = Some(Box::new(callback)))
    }
}

impl View for KnobView {
    fn element(&self) -> Option<&'static str> {
        Some("spike-knob")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, _| match window_event {
            WindowEvent::MouseDown(button) if *button == MouseButton::Left => {
                self.dragging = true;
                self.prev_y = cx.mouse().left.pos_down.1;
                cx.capture();
                cx.focus_with_visibility(false);
            }
            WindowEvent::MouseUp(button) if *button == MouseButton::Left => {
                self.dragging = false;
                cx.release();
            }
            WindowEvent::MouseMove(_, y) if self.dragging => {
                let delta = (self.prev_y - *y) / DRAG_RANGE_PX;
                self.prev_y = *y;
                let norm = (self.norm + delta).clamp(0.0, 1.0);
                if let Some(callback) = &self.on_change {
                    (callback)(cx, norm);
                }
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &Canvas) {
        let bounds = cx.bounds();
        let center_x = bounds.x + bounds.w / 2.0;
        let center_y = bounds.y + bounds.h / 2.0;
        let radius = bounds.w.min(bounds.h) / 2.0 - 8.0;
        let oval = vg::Rect::from_xywh(
            center_x - radius,
            center_y - radius,
            2.0 * radius,
            2.0 * radius,
        );

        let stroke = |color: vg::Color, width: f32| {
            let mut paint = vg::Paint::default();
            paint.set_anti_alias(true);
            paint.set_style(vg::PaintStyle::Stroke);
            paint.set_stroke_width(width);
            paint.set_stroke_cap(vg::PaintCap::Round);
            paint.set_color(color);
            paint
        };

        // Track, then the value arc on top of it.
        canvas.draw_arc(
            oval,
            SWEEP_START_DEG,
            SWEEP_DEG,
            false,
            &stroke(track(), 7.0),
        );
        if self.norm > 0.001 {
            canvas.draw_arc(
                oval,
                SWEEP_START_DEG,
                SWEEP_DEG * self.norm,
                false,
                &stroke(accent(), 7.0),
            );
        }

        // Pointer line.
        let angle = (SWEEP_START_DEG + SWEEP_DEG * self.norm).to_radians();
        let tip = |r: f32| vg::Point::new(center_x + r * angle.cos(), center_y + r * angle.sin());
        canvas.draw_line(
            tip(radius * 0.35),
            tip(radius * 0.82),
            &stroke(bright(), 4.0),
        );
    }
}

// --- peak meter ---------------------------------------------------------------

/// One horizontal peak bar over a -60..0 dBFS scale, hot zone above -6 dB.
/// (Text labels live outside as `Label` views — drawing text via skia needs
/// font plumbing, which is itself a spike data point.)
pub struct MeterView {
    norm: f32,
}

impl MeterView {
    pub fn new(cx: &mut Context, value: ReadSignal<f32>) -> Handle<'_, Self> {
        Self { norm: value.get() }
            .build(cx, |_| {})
            .bind(value, move |handle| {
                let mut handle = handle.modify(|meter| meter.norm = value.get());
                handle.needs_redraw();
            })
    }
}

impl View for MeterView {
    fn element(&self) -> Option<&'static str> {
        Some("spike-meter")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &Canvas) {
        let bounds = cx.bounds();
        let hot_norm = (HOT_DB - METER_FLOOR_DB) / -METER_FLOOR_DB;

        let fill = |color: vg::Color| {
            let mut paint = vg::Paint::default();
            paint.set_anti_alias(true);
            paint.set_style(vg::PaintStyle::Fill);
            paint.set_color(color);
            paint
        };
        let rect = |x: f32, w: f32| vg::Rect::from_xywh(x, bounds.y, w, bounds.h);

        canvas.draw_rect(
            rect(bounds.x, bounds.w),
            &fill(vg::Color::from_argb(255, 36, 36, 43)),
        );
        let ok = self.norm.min(hot_norm);
        if ok > 0.0 {
            canvas.draw_rect(
                rect(bounds.x, bounds.w * ok),
                &fill(vg::Color::from_argb(255, 77, 199, 107)),
            );
        }
        if self.norm > hot_norm {
            canvas.draw_rect(
                rect(
                    bounds.x + bounds.w * hot_norm,
                    bounds.w * (self.norm - hot_norm),
                ),
                &fill(vg::Color::from_argb(255, 235, 66, 54)),
            );
        }

        // dB scale ticks under the bar (numbers omitted: skia text needs fonts).
        let tick = fill(vg::Color::from_argb(255, 140, 140, 153));
        for db in [-40.0f32, -20.0, HOT_DB] {
            let norm = (db - METER_FLOOR_DB) / -METER_FLOOR_DB;
            let x = bounds.x + bounds.w * norm;
            canvas.draw_rect(
                vg::Rect::from_xywh(x - 0.5, bounds.y + bounds.h - 5.0, 1.0, 5.0),
                &tick,
            );
        }
    }
}
