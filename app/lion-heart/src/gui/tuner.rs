//! The tuner display: a big note readout over a ±50-cent scale with a
//! glowing needle. Green (and an IN TUNE badge) within ±3 cents, amber
//! otherwise. Pitch detection itself lives in `lh_dsp::tuner`; this canvas
//! only renders the latest reading.

use iced::widget::canvas;
use iced::widget::text::Alignment as TextAlign;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::Message;
use super::theme::{ACCENT, INSET, METER_OK, PANEL_HI, TEXT_BRIGHT, TEXT_DIM, TRACK, dim};

/// In tune when the needle is within this many cents.
const IN_TUNE_CENTS: f32 = 3.0;

#[derive(Debug, Clone)]
pub struct Reading {
    pub note: String,
    pub octave: i32,
    pub cents: f32,
    pub freq_hz: f32,
}

pub struct TunerDisplay<'a> {
    pub reading: Option<Reading>,
    pub cache: &'a canvas::Cache,
}

impl canvas::Program<Message> for TunerDisplay<'_> {
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
            let cx = frame.width() / 2.0;
            let scale_w = (frame.width() - 120.0).min(520.0);
            let scale_y = frame.height() / 2.0 + 56.0;
            let x_at = |cents: f32| cx + cents / 50.0 * (scale_w / 2.0);

            let text = |content: String, pos: Point, color: Color, size: f32| canvas::Text {
                content,
                position: pos,
                color,
                size: Pixels(size),
                font: Font::MONOSPACE,
                align_x: TextAlign::Center,
                ..canvas::Text::default()
            };

            // Scale bed and cent ticks (every 5, taller every 25).
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    Point::new(x_at(-50.0), scale_y - 2.0),
                    Size::new(scale_w, 4.0),
                    2.0.into(),
                ),
                INSET,
            );
            let mut cents = -50.0f32;
            while cents <= 50.0 {
                let major = cents.rem_euclid(25.0) == 0.0;
                let zero = cents == 0.0;
                let h = if zero {
                    22.0
                } else if major {
                    14.0
                } else {
                    7.0
                };
                frame.fill_rectangle(
                    Point::new(x_at(cents) - 0.75, scale_y - h - 4.0),
                    Size::new(1.5, h),
                    if zero {
                        TEXT_DIM
                    } else if major {
                        dim(TEXT_DIM, 0.8)
                    } else {
                        dim(TRACK, 0.9)
                    },
                );
                if major {
                    frame.fill_text(text(
                        format!("{cents:+.0}"),
                        Point::new(x_at(cents), scale_y + 10.0),
                        TEXT_DIM,
                        11.0,
                    ));
                }
                cents += 5.0;
            }

            match &self.reading {
                Some(r) => {
                    let in_tune = r.cents.abs() <= IN_TUNE_CENTS;
                    let color = if in_tune { METER_OK } else { ACCENT };

                    // The note, huge; octave rides small beside it.
                    frame.fill_text(text(
                        format!("{}{}", r.note, r.octave),
                        Point::new(cx, scale_y - 210.0),
                        if in_tune { METER_OK } else { TEXT_BRIGHT },
                        88.0,
                    ));
                    frame.fill_text(text(
                        format!("{:+.0} cents · {:.1} Hz", r.cents, r.freq_hz),
                        Point::new(cx, scale_y - 100.0),
                        TEXT_DIM,
                        14.0,
                    ));
                    if in_tune {
                        let badge = Size::new(84.0, 22.0);
                        let at = Point::new(cx - badge.width / 2.0, scale_y - 66.0);
                        frame.fill(
                            &canvas::Path::rounded_rectangle(at, badge, 11.0.into()),
                            dim(METER_OK, 0.15),
                        );
                        frame.fill_text(text(
                            "IN TUNE".into(),
                            Point::new(cx, scale_y - 61.0),
                            METER_OK,
                            12.0,
                        ));
                    }

                    // Needle with a soft glow.
                    let x = x_at(r.cents.clamp(-50.0, 50.0));
                    frame.fill_rectangle(
                        Point::new(x - 6.0, scale_y - 38.0),
                        Size::new(12.0, 42.0),
                        dim(color, 0.18),
                    );
                    frame.fill_rectangle(
                        Point::new(x - 2.0, scale_y - 36.0),
                        Size::new(4.0, 40.0),
                        color,
                    );
                }
                None => {
                    frame.fill_text(text(
                        "—".into(),
                        Point::new(cx, scale_y - 210.0),
                        PANEL_HI,
                        88.0,
                    ));
                    frame.fill_text(text(
                        "play a single string".into(),
                        Point::new(cx, scale_y - 100.0),
                        TEXT_DIM,
                        14.0,
                    ));
                }
            }
        });
        vec![geometry]
    }
}
