//! Input/output peak meters and their display ballistics.

use iced::widget::canvas;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};
use lh_core::lin_to_db;

use super::Message;
use super::theme::{METER_HOT, METER_OK, PANEL_HI, TEXT_DIM};

/// Meter floor; peaks below this render as an empty bar.
pub const FLOOR_DB: f32 = -60.0;
/// Bars turn red above this level.
const HOT_DB: f32 = -6.0;
/// Display fall rate in dB per UI frame (~30 dB/s at 60 fps), instant attack.
const FALL_DB_PER_FRAME: f32 = 0.5;

/// Instant attack, constant-rate fall — shared by the header meters.
pub struct Ballistics {
    in_db: f32,
    out_db: f32,
}

impl Ballistics {
    pub fn new() -> Self {
        Self {
            in_db: FLOOR_DB,
            out_db: FLOOR_DB,
        }
    }

    /// Feed the latest linear peaks once per frame.
    pub fn tick(&mut self, peak_in: f32, peak_out: f32) {
        let feed = |shown: &mut f32, peak: f32| {
            let db = lin_to_db(peak.max(1e-6)).max(FLOOR_DB);
            *shown = if db >= *shown {
                db
            } else {
                (*shown - FALL_DB_PER_FRAME).max(db)
            };
        };
        feed(&mut self.in_db, peak_in);
        feed(&mut self.out_db, peak_out);
    }

    /// Current display values as 0..1 bar lengths (floor..0 dBFS).
    pub fn norms(&self) -> (f32, f32) {
        let n = |db: f32| ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        (n(self.in_db), n(self.out_db))
    }
}

/// Two slim horizontal bars (IN over OUT) with a -6 dB hot zone.
pub struct Meters<'a> {
    pub norms: (f32, f32),
    pub cache: &'a canvas::Cache,
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
            let label_w = 30.0;
            let bar_w = frame.width() - label_w;
            let bar_h = 12.0;
            let hot = (HOT_DB - FLOOR_DB) / -FLOOR_DB;

            let mut draw_bar = |y: f32, label: &str, norm: f32| {
                frame.fill_text(canvas::Text {
                    content: label.into(),
                    position: Point::new(0.0, y),
                    color: TEXT_DIM,
                    size: Pixels(11.0),
                    font: Font::MONOSPACE,
                    ..canvas::Text::default()
                });
                frame.fill_rectangle(Point::new(label_w, y), Size::new(bar_w, bar_h), PANEL_HI);
                let ok = norm.min(hot);
                if ok > 0.0 {
                    frame.fill_rectangle(
                        Point::new(label_w, y),
                        Size::new(bar_w * ok, bar_h),
                        METER_OK,
                    );
                }
                if norm > hot {
                    frame.fill_rectangle(
                        Point::new(label_w + bar_w * hot, y),
                        Size::new(bar_w * (norm - hot), bar_h),
                        METER_HOT,
                    );
                }
                // -6 dB tick.
                frame.fill_rectangle(
                    Point::new(label_w + bar_w * hot - 0.5, y),
                    Size::new(1.0, bar_h),
                    Color { a: 0.6, ..TEXT_DIM },
                );
            };
            draw_bar(4.0, "IN", self.norms.0);
            draw_bar(4.0 + bar_h + 8.0, "OUT", self.norms.1);
        });
        vec![geometry]
    }
}
