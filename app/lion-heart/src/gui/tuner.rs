//! The tuner display: note name, a ±50-cent scale with a needle, frequency
//! readout. Green when within ±3 cents. Pitch detection itself lives in
//! `lh_dsp::tuner`; this canvas only renders the latest reading.

use iced::widget::canvas;
use iced::widget::text::Alignment as TextAlign;
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::Message;
use super::theme::{ACCENT, METER_OK, PANEL_HI, TEXT_BRIGHT, TEXT_DIM, TRACK};

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
            let scale_w = (frame.width() - 80.0).min(460.0);
            let scale_y = frame.height() / 2.0 + 40.0;
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

            // Scale line and cent ticks.
            frame.fill_rectangle(
                Point::new(x_at(-50.0), scale_y),
                Size::new(scale_w, 2.0),
                TRACK,
            );
            for cents in [-50.0f32, -25.0, -10.0, 0.0, 10.0, 25.0, 50.0] {
                let major = cents == 0.0;
                let h = if major { 18.0 } else { 10.0 };
                frame.fill_rectangle(
                    Point::new(x_at(cents) - 0.75, scale_y - h),
                    Size::new(1.5, h + 2.0),
                    if major { TEXT_DIM } else { TRACK },
                );
                frame.fill_text(text(
                    format!("{cents:+.0}"),
                    Point::new(x_at(cents), scale_y + 10.0),
                    TEXT_DIM,
                    11.0,
                ));
            }

            match &self.reading {
                Some(r) => {
                    let in_tune = r.cents.abs() <= IN_TUNE_CENTS;
                    let color = if in_tune { METER_OK } else { ACCENT };

                    frame.fill_text(text(
                        format!("{}{}", r.note, r.octave),
                        Point::new(cx, scale_y - 150.0),
                        if in_tune { METER_OK } else { TEXT_BRIGHT },
                        64.0,
                    ));
                    frame.fill_text(text(
                        format!("{:+.0} cents · {:.1} Hz", r.cents, r.freq_hz),
                        Point::new(cx, scale_y - 66.0),
                        TEXT_DIM,
                        14.0,
                    ));

                    // Needle.
                    let x = x_at(r.cents.clamp(-50.0, 50.0));
                    frame.fill_rectangle(
                        Point::new(x - 2.0, scale_y - 34.0),
                        Size::new(4.0, 36.0),
                        color,
                    );
                }
                None => {
                    frame.fill_text(text(
                        "—".into(),
                        Point::new(cx, scale_y - 150.0),
                        PANEL_HI,
                        64.0,
                    ));
                    frame.fill_text(text(
                        "play a single string".into(),
                        Point::new(cx, scale_y - 66.0),
                        TEXT_DIM,
                        14.0,
                    ));
                }
            }
        });
        vec![geometry]
    }
}
