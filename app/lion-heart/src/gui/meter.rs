//! Input/output peak meters and their display ballistics: three-zone bars
//! (green / amber / red) in a recessed trough, with a peak-hold tick that
//! rides the recent maximum — glanceable gain staging from across a room.

use iced::widget::canvas;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};
use lh_core::lin_to_db;

use super::Message;
use super::theme::{ACCENT, INSET, METER_HOT, METER_OK, TEXT_DIM, dim};

/// Meter floor; peaks below this render as an empty bar.
pub const FLOOR_DB: f32 = -60.0;
/// Bars turn amber above this level…
const MID_DB: f32 = -18.0;
/// …and red above this one.
const HOT_DB: f32 = -6.0;
/// Display fall rate in dB per UI frame (~30 dB/s at 60 fps), instant attack.
const FALL_DB_PER_FRAME: f32 = 0.5;
/// Frames the peak-hold tick stays put before it starts falling.
const HOLD_FRAMES: u32 = 45;

fn norm_of(db: f32) -> f32 {
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// One channel's display state: the bar level plus its peak-hold tick.
struct Channel {
    db: f32,
    hold_db: f32,
    hold_age: u32,
}

impl Channel {
    fn new() -> Self {
        Self {
            db: FLOOR_DB,
            hold_db: FLOOR_DB,
            hold_age: 0,
        }
    }

    fn feed(&mut self, peak: f32) {
        let db = lin_to_db(peak.max(1e-6)).max(FLOOR_DB);
        self.db = if db >= self.db {
            db
        } else {
            (self.db - FALL_DB_PER_FRAME).max(db)
        };
        if db >= self.hold_db {
            self.hold_db = db;
            self.hold_age = 0;
        } else {
            self.hold_age += 1;
            if self.hold_age > HOLD_FRAMES {
                self.hold_db = (self.hold_db - 2.0 * FALL_DB_PER_FRAME).max(self.db);
            }
        }
    }
}

/// Instant attack, constant-rate fall — shared by the header meters.
pub struct Ballistics {
    input: Channel,
    output: Channel,
}

impl Ballistics {
    pub fn new() -> Self {
        Self {
            input: Channel::new(),
            output: Channel::new(),
        }
    }

    /// Feed the latest linear peaks once per frame.
    pub fn tick(&mut self, peak_in: f32, peak_out: f32) {
        self.input.feed(peak_in);
        self.output.feed(peak_out);
    }

    /// Current display values as 0..1 bar lengths (floor..0 dBFS).
    pub fn norms(&self) -> (f32, f32) {
        (norm_of(self.input.db), norm_of(self.output.db))
    }

    /// Peak-hold tick positions, same scale.
    pub fn holds(&self) -> (f32, f32) {
        (norm_of(self.input.hold_db), norm_of(self.output.hold_db))
    }
}

/// Two slim horizontal bars (IN over OUT) with green/amber/red zones.
pub struct Meters<'a> {
    pub norms: (f32, f32),
    pub holds: (f32, f32),
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
            let bar_h = 11.0;
            let mid = norm_of(MID_DB);
            let hot = norm_of(HOT_DB);

            let mut draw_bar = |y: f32, label: &str, norm: f32, hold: f32| {
                frame.fill_text(canvas::Text {
                    content: label.into(),
                    position: Point::new(0.0, y - 1.0),
                    color: TEXT_DIM,
                    size: Pixels(10.0),
                    font: Font::MONOSPACE,
                    ..canvas::Text::default()
                });
                // Recessed trough.
                frame.fill(
                    &canvas::Path::rounded_rectangle(
                        Point::new(label_w, y),
                        Size::new(bar_w, bar_h),
                        3.0.into(),
                    ),
                    INSET,
                );
                // Zoned fill: green to -18, amber to -6, red above.
                let zones: [(f32, f32, Color); 3] = [
                    (0.0, mid, METER_OK),
                    (mid, hot, ACCENT),
                    (hot, 1.0, METER_HOT),
                ];
                for (from, to, color) in zones {
                    let end = norm.min(to);
                    if end > from {
                        frame.fill_rectangle(
                            Point::new(label_w + bar_w * from, y + 1.0),
                            Size::new(bar_w * (end - from), bar_h - 2.0),
                            color,
                        );
                    }
                }
                // Peak-hold tick.
                if hold > 0.005 {
                    let color = if hold >= hot {
                        METER_HOT
                    } else if hold >= mid {
                        ACCENT
                    } else {
                        METER_OK
                    };
                    frame.fill_rectangle(
                        Point::new(label_w + (bar_w - 2.0) * hold, y),
                        Size::new(2.0, bar_h),
                        color,
                    );
                }
                // -6 dB gridline.
                frame.fill_rectangle(
                    Point::new(label_w + bar_w * hot - 0.5, y),
                    Size::new(1.0, bar_h),
                    dim(TEXT_DIM, 0.5),
                );
            };
            draw_bar(4.0, "IN", self.norms.0, self.holds.0);
            draw_bar(4.0 + bar_h + 7.0, "OUT", self.norms.1, self.holds.1);
        });
        vec![geometry]
    }
}
