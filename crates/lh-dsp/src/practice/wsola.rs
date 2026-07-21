//! WSOLA time-stretch (PRD 019, Phase 3).
//!
//! Waveform-Similarity Overlap-Add: change a signal's **duration** (tempo)
//! while preserving its **pitch**. Output is built one grain at a time by
//! overlap-adding windowed source segments; before each grain a short
//! cross-correlation search picks the segment whose overlap region best matches
//! the previous grain's natural continuation, which is what keeps the pitch
//! period intact (plain OLA would smear it). Stereo-linked: the alignment offset
//! is chosen once from the L channel and applied to both, so the image holds.
//!
//! Streaming: [`Wsola::fill`] reads from a caller-owned source buffer at an
//! internal analysis cursor and emits stretched output; the song player
//! (`SongPlayer`) owns the source and the loop logic. Off the audio thread
//! (player thread), but allocation-free per call all the same.

/// Grain (window) length at 48 kHz — ~21 ms, enough to hold a low-note period
/// for the correlation to lock onto. Scaled by sample rate in `prepare`.
const WIN_48K: usize = 1_024;
/// Correlation search radius at 48 kHz (± samples around the target).
const SEARCH_48K: usize = 256;

/// A stereo WSOLA time-stretcher.
pub struct Wsola {
    win: usize,
    /// Synthesis hop = half the window (50 % overlap); also the overlap length.
    hop: usize,
    search: usize,
    window: Vec<f32>,

    /// Analysis cursor in source frames (advances by `hop * speed` per grain).
    pos: f64,
    /// Windowed overlap tails carried between grains (length `hop`).
    tail_l: Vec<f32>,
    tail_r: Vec<f32>,
    /// The previous grain's natural continuation (mono), the correlation target.
    natural: Vec<f32>,
    primed: bool,

    /// Output that has been synthesised but not yet handed back (one grain max).
    queue_l: Vec<f32>,
    queue_r: Vec<f32>,
    queue_head: usize,
}

impl Default for Wsola {
    fn default() -> Self {
        Self::new()
    }
}

impl Wsola {
    pub fn new() -> Self {
        let mut w = Self {
            win: WIN_48K,
            hop: WIN_48K / 2,
            search: SEARCH_48K,
            window: Vec::new(),
            pos: 0.0,
            tail_l: Vec::new(),
            tail_r: Vec::new(),
            natural: Vec::new(),
            primed: false,
            queue_l: Vec::new(),
            queue_r: Vec::new(),
            queue_head: 0,
        };
        w.prepare(48_000.0);
        w
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        let scale = (sample_rate / 48_000.0).max(0.25);
        self.win = ((WIN_48K as f32 * scale) as usize).max(128) & !1; // even
        self.hop = self.win / 2;
        self.search = ((SEARCH_48K as f32 * scale) as usize).max(32);
        // Periodic Hann window.
        self.window = (0..self.win)
            .map(|n| {
                let x = std::f32::consts::TAU * n as f32 / self.win as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        self.tail_l = vec![0.0; self.hop];
        self.tail_r = vec![0.0; self.hop];
        self.natural = vec![0.0; self.hop];
        self.queue_l = vec![0.0; self.hop];
        self.queue_r = vec![0.0; self.hop];
        self.reset(0.0);
    }

    /// Restart at source frame `pos`, dropping all overlap state (a seam — the
    /// caller uses this at a loop point / seek).
    pub fn reset(&mut self, pos: f64) {
        self.pos = pos;
        self.primed = false;
        self.tail_l.fill(0.0);
        self.tail_r.fill(0.0);
        self.natural.fill(0.0);
        self.queue_head = self.hop; // empty
    }

    /// Current analysis position in source frames (leads the emitted audio by
    /// roughly one window — close enough for a progress readout).
    pub fn pos(&self) -> f64 {
        self.pos
    }

    /// Fill `out_l`/`out_r` (equal length) with source stretched by `speed`
    /// (`0.5` = half speed / twice as long, pitch unchanged; `1.0` = neutral).
    /// Reads `src_l`/`src_r` around the internal cursor; out-of-range reads are
    /// zero (so the tail past the source fades out).
    pub fn fill(
        &mut self,
        src_l: &[f32],
        src_r: &[f32],
        speed: f32,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let ha = self.hop as f64 * speed.max(0.01) as f64;
        for i in 0..out_l.len() {
            if self.queue_head >= self.hop {
                self.synthesize_grain(src_l, src_r, ha);
                self.queue_head = 0;
            }
            out_l[i] = self.queue_l[self.queue_head];
            out_r[i] = self.queue_r[self.queue_head];
            self.queue_head += 1;
        }
    }

    /// Produce one `hop`-length output grain into the queue, advancing `pos`.
    fn synthesize_grain(&mut self, src_l: &[f32], src_r: &[f32], ha: f64) {
        let base = self.pos.round() as isize;
        // Search for the offset whose overlap region best matches the previous
        // grain's natural continuation (skip on the first grain).
        let delta = if self.primed {
            self.best_offset(src_l, base)
        } else {
            0
        };
        let start = base + delta;

        // Overlap-add: first `hop` windowed samples add onto the tail and are
        // emitted; the last `hop` become the new tail.
        for k in 0..self.hop {
            let w0 = self.window[k];
            let w1 = self.window[k + self.hop];
            let g0_l = read(src_l, start + k as isize) * w0;
            let g0_r = read(src_r, start + k as isize) * w0;
            self.queue_l[k] = self.tail_l[k] + g0_l;
            self.queue_r[k] = self.tail_r[k] + g0_r;
            self.tail_l[k] = read(src_l, start + (k + self.hop) as isize) * w1;
            self.tail_r[k] = read(src_r, start + (k + self.hop) as isize) * w1;
        }
        // The next grain's overlap region should match this grain's tail region
        // (mono reference, un-windowed).
        for k in 0..self.hop {
            self.natural[k] = read(src_l, start + (k + self.hop) as isize);
        }
        self.primed = true;
        self.pos += ha;
    }

    /// Offset in `[-search, search]` maximising normalised cross-correlation
    /// between the candidate grain's overlap region and `natural`.
    fn best_offset(&self, src_l: &[f32], base: isize) -> isize {
        let n = self.hop;
        let nat_energy: f32 = self.natural.iter().map(|s| s * s).sum::<f32>().max(1e-9);
        let mut best_delta = 0isize;
        let mut best_score = f32::NEG_INFINITY;
        let search = self.search as isize;
        let mut delta = -search;
        while delta <= search {
            let mut dot = 0.0f32;
            let mut energy = 0.0f32;
            for k in 0..n {
                let s = read(src_l, base + delta + k as isize);
                dot += s * self.natural[k];
                energy += s * s;
            }
            // Normalised: reward similar shape, not just loud regions.
            let score = dot / (energy.max(1e-9) * nat_energy).sqrt();
            if score > best_score {
                best_score = score;
                best_delta = delta;
            }
            delta += 1;
        }
        best_delta
    }
}

/// Read a source sample, zero outside `[0, len)`.
#[inline]
fn read(src: &[f32], i: isize) -> f32 {
    if i >= 0 && (i as usize) < src.len() {
        src[i as usize]
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goertzel magnitude of `freq` in `block` at `sr`.
    fn goertzel(block: &[f32], sr: f32, freq: f32) -> f32 {
        let w = std::f32::consts::TAU * freq / sr;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in block {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).sqrt() / block.len() as f32
    }

    fn sine(sr: f32, hz: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / sr).sin())
            .collect()
    }

    /// Stretch a stereo sine by `speed`, return the mono-L output.
    fn stretch(speed: f32, in_frames: usize) -> Vec<f32> {
        let sr = 48_000.0;
        let src = sine(sr, 220.0, in_frames);
        let mut w = Wsola::new();
        w.prepare(sr);
        // Ask for the expected output length: in_frames / speed.
        let out_frames = (in_frames as f32 / speed) as usize;
        let mut out_l = vec![0.0; out_frames];
        let mut out_r = vec![0.0; out_frames];
        w.fill(&src, &src, speed, &mut out_l, &mut out_r);
        out_l
    }

    #[test]
    fn half_speed_preserves_pitch() {
        // speed 0.5 → twice as long, still 220 Hz.
        let out = stretch(0.5, 48_000);
        // Skip the priming region; measure the settled middle.
        let mid = &out[20_000..70_000];
        let f220 = goertzel(mid, 48_000.0, 220.0);
        let f110 = goertzel(mid, 48_000.0, 110.0);
        let f440 = goertzel(mid, 48_000.0, 440.0);
        assert!(f220 > 0.2, "220 Hz must survive the stretch: {f220}");
        assert!(
            f220 > f110 * 4.0,
            "no octave-down artifact: {f220} vs {f110}"
        );
        assert!(f220 > f440 * 4.0, "no octave-up artifact: {f220} vs {f440}");
    }

    #[test]
    fn double_speed_preserves_pitch() {
        // speed 2.0 → half as long, still 220 Hz.
        let out = stretch(2.0, 96_000);
        let mid = &out[8_000..40_000];
        let f220 = goertzel(mid, 48_000.0, 220.0);
        let f110 = goertzel(mid, 48_000.0, 110.0);
        assert!(f220 > 0.2, "220 Hz must survive: {f220}");
        assert!(
            f220 > f110 * 4.0,
            "pitch, not period-doubled: {f220} vs {f110}"
        );
    }

    #[test]
    fn neutral_speed_is_roughly_transparent() {
        let out = stretch(1.0, 48_000);
        let mid = &out[10_000..38_000];
        let f220 = goertzel(mid, 48_000.0, 220.0);
        assert!(f220 > 0.3, "neutral stretch keeps the tone: {f220}");
    }

    #[test]
    fn output_is_finite_and_bounded() {
        for &speed in &[0.25f32, 0.5, 0.8, 1.0, 1.5, 2.0] {
            let out = stretch(speed, 48_000);
            assert!(out.iter().all(|s| s.is_finite()), "finite @ {speed}");
            // Overlap-add of a unit sine stays within a small factor of 1.
            assert!(out.iter().all(|s| s.abs() < 2.0), "bounded @ {speed}");
        }
    }

    #[test]
    fn advances_the_cursor_by_speed() {
        let sr = 48_000.0;
        let src = sine(sr, 220.0, 48_000);
        let mut w = Wsola::new();
        w.prepare(sr);
        let mut out_l = vec![0.0; 10_000];
        let mut out_r = vec![0.0; 10_000];
        w.fill(&src, &src, 0.5, &mut out_l, &mut out_r);
        // 10k output samples at half speed → ~5k source frames consumed.
        assert!(
            (w.pos() - 5_000.0).abs() < 600.0,
            "cursor at {} after 10k out @ 0.5x",
            w.pos()
        );
    }
}
