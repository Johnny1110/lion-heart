//! Song-player waveform strip (PRD 019, Phase 3): a peak envelope with the
//! play cursor and the A-B loop region shaded. Draw-only — seeking is the
//! slider beneath it; this is the visual reference.

use iced::widget::canvas;
use iced::{Point, Rectangle, Renderer, Size, Theme, mouse};

use super::Message;
use super::theme;

/// A precomputed peak envelope plus the live playhead and loop markers.
pub struct Waveform<'a> {
    /// Max |sample| per horizontal bucket, `0..1`.
    pub peaks: &'a [f32],
    /// Play position as a fraction `0..1`.
    pub position: f32,
    /// A-B loop as fractions `0..1`, if set.
    pub loop_range: Option<(f32, f32)>,
}

impl canvas::Program<Message> for Waveform<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // No cache: the playhead moves every frame, so redraw fresh (a few
        // hundred bars is cheap).
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = frame.width();
        let h = frame.height();
        let mid = h * 0.5;

        // Recessed well.
        frame.fill(
            &canvas::Path::rounded_rectangle(Point::ORIGIN, Size::new(w, h), 6.0.into()),
            theme::INSET,
        );

        // Loop region.
        if let Some((a, b)) = self.loop_range {
            let (x0, x1) = (a.clamp(0.0, 1.0) * w, b.clamp(0.0, 1.0) * w);
            if x1 > x0 {
                frame.fill(
                    &canvas::Path::rectangle(Point::new(x0, 0.0), Size::new(x1 - x0, h)),
                    theme::dim(theme::ACCENT, 0.16),
                );
            }
        }

        // Peaks as a filled envelope (a slim bar per bucket).
        let n = self.peaks.len().max(1);
        let bar_w = (w / n as f32).max(1.0);
        for (i, &p) in self.peaks.iter().enumerate() {
            let x = i as f32 / n as f32 * w;
            let bh = (p.clamp(0.0, 1.0) * (h * 0.44)).max(0.5);
            // Played portion is brighter than the rest.
            let played = (i as f32 / n as f32) <= self.position;
            let color = if played {
                theme::dim(theme::ACCENT, 0.85)
            } else {
                theme::dim(theme::TEXT_DIM, 0.6)
            };
            frame.fill(
                &canvas::Path::rectangle(Point::new(x, mid - bh), Size::new(bar_w, bh * 2.0)),
                color,
            );
        }

        // Playhead.
        let cx = self.position.clamp(0.0, 1.0) * w;
        frame.stroke(
            &canvas::Path::line(Point::new(cx, 0.0), Point::new(cx, h)),
            canvas::Stroke::default()
                .with_color(theme::TEXT_BRIGHT)
                .with_width(2.0),
        );

        vec![frame.into_geometry()]
    }
}
