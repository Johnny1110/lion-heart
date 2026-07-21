//! Song player (PRD 019, Phase 3): play a decoded backing track along with the
//! guitar, for practice.
//!
//! Pipeline per block: **WSOLA** varispeed (change tempo, keep pitch) →
//! **GrainShift** transpose (change pitch ±semitones, keep tempo) → mix level.
//! Splitting the two features across the two granular tools (rather than a
//! single combined stretch) keeps each independently correct and testable, and
//! reuses the pitch shifter the octaver already ships (ADR 016). An **A-B loop**
//! reads a sub-range on repeat.
//!
//! The decoded audio is a [`SongBuffer`] the app's loader fills (via
//! `symphonia`, off the audio thread); this module is pure DSP — no I/O. Runs on
//! the player thread and is allocation-free per block.

use std::sync::Arc;

use crate::blocks::grain::GrainShift;

use super::Wsola;

/// A decoded, resampled-to-engine-rate stereo backing track. Immutable once
/// built (shared to the player via `Arc`).
pub struct SongBuffer {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub sample_rate: u32,
}

impl SongBuffer {
    pub fn frames(&self) -> usize {
        self.l.len().min(self.r.len())
    }

    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.max(1) as f32
    }

    /// A downsampled peak envelope (max |sample| per bucket) for a waveform
    /// display. Computed once on load, off the audio thread.
    pub fn peaks(&self, buckets: usize) -> Vec<f32> {
        let frames = self.frames();
        if frames == 0 || buckets == 0 {
            return vec![0.0; buckets];
        }
        let per = frames.div_ceil(buckets).max(1);
        (0..buckets)
            .map(|b| {
                let start = (b * per).min(frames);
                let end = (start + per).min(frames);
                self.l[start..end]
                    .iter()
                    .zip(&self.r[start..end])
                    .fold(0.0f32, |m, (l, r)| m.max(l.abs()).max(r.abs()))
            })
            .collect()
    }
}

/// Sub-chunk the render loop into pieces this small so the A-B loop / end is
/// caught within a few ms rather than a whole block.
const SUB: usize = 128;

/// The practice song player.
pub struct SongPlayer {
    song: Option<Arc<SongBuffer>>,
    playing: bool,
    /// Tempo ratio: 1.0 = original, 0.5 = half speed (same pitch).
    speed: f32,
    /// Transpose in semitones (−12..12), 0 = none.
    semitones: f32,
    mix: f32,
    /// A-B loop in source frames; `b == 0` means "no loop, play to the end".
    loop_a: usize,
    loop_b: usize,

    wsola: Wsola,
    shift_l: GrainShift,
    shift_r: GrainShift,
    tmp_l: Vec<f32>,
    tmp_r: Vec<f32>,
}

impl Default for SongPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl SongPlayer {
    pub fn new() -> Self {
        let mut p = Self {
            song: None,
            playing: false,
            speed: 1.0,
            semitones: 0.0,
            mix: 0.7,
            loop_a: 0,
            loop_b: 0,
            wsola: Wsola::new(),
            shift_l: GrainShift::new(),
            shift_r: GrainShift::new(),
            tmp_l: Vec::new(),
            tmp_r: Vec::new(),
        };
        p.prepare(48_000);
        p
    }

    pub fn prepare(&mut self, sample_rate: u32) {
        let sr = sample_rate.max(1) as f32;
        self.wsola.prepare(sr);
        self.shift_l.prepare(sr);
        self.shift_r.prepare(sr);
        self.tmp_l = vec![0.0; SUB];
        self.tmp_r = vec![0.0; SUB];
    }

    /// Load a decoded song (resets playback to the start, stopped).
    pub fn set_song(&mut self, song: Arc<SongBuffer>) {
        self.song = Some(song);
        self.playing = false;
        self.loop_a = 0;
        self.loop_b = 0;
        self.seek(0);
    }

    pub fn clear_song(&mut self) {
        self.song = None;
        self.playing = false;
    }

    pub fn has_song(&self) -> bool {
        self.song.is_some()
    }

    pub fn song_frames(&self) -> usize {
        self.song.as_ref().map_or(0, |s| s.frames())
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn play(&mut self) {
        if self.song.is_some() {
            self.playing = true;
        }
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    /// Jump to a source frame, dropping stretch/shift state (a seam).
    pub fn seek(&mut self, frame: usize) {
        let frame = frame.min(self.song_frames());
        self.wsola.reset(frame as f64);
        self.shift_l.clear();
        self.shift_r.clear();
    }

    /// Current play position in source frames.
    pub fn pos_frames(&self) -> usize {
        (self.wsola.pos().max(0.0) as usize).min(self.song_frames())
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 2.0);
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn set_semitones(&mut self, semitones: f32) {
        self.semitones = semitones.clamp(-12.0, 12.0);
    }
    pub fn semitones(&self) -> f32 {
        self.semitones
    }

    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }
    pub fn mix(&self) -> f32 {
        self.mix
    }

    /// Set an A-B loop over `[a, b)` source frames (`b <= a` clears it).
    pub fn set_loop(&mut self, a: usize, b: usize) {
        let frames = self.song_frames();
        let a = a.min(frames);
        let b = b.min(frames);
        if b > a {
            self.loop_a = a;
            self.loop_b = b;
            // If the cursor is outside the new loop, drop it to A.
            if self.pos_frames() < a || self.pos_frames() >= b {
                self.seek(a);
            }
        } else {
            self.loop_a = 0;
            self.loop_b = 0;
        }
    }

    pub fn loop_range(&self) -> Option<(usize, usize)> {
        (self.loop_b > self.loop_a).then_some((self.loop_a, self.loop_b))
    }

    /// Render one block, playing the song (silence when stopped / no song).
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        if !self.playing {
            out_l.fill(0.0);
            out_r.fill(0.0);
            return;
        }
        let Some(song) = self.song.clone() else {
            out_l.fill(0.0);
            out_r.fill(0.0);
            return;
        };
        let ratio = 2f32.powf(self.semitones / 12.0);
        let transpose = (self.semitones).abs() > 1e-3;

        let mut i = 0;
        while i < out_l.len() {
            let end = (i + SUB).min(out_l.len());
            let n = end - i;
            self.wsola.fill(
                &song.l,
                &song.r,
                self.speed,
                &mut self.tmp_l[..n],
                &mut self.tmp_r[..n],
            );
            for k in 0..n {
                let (mut l, mut r) = (self.tmp_l[k], self.tmp_r[k]);
                if transpose {
                    l = self.shift_l.process(l, ratio);
                    r = self.shift_r.process(r, ratio);
                }
                out_l[i + k] = l * self.mix;
                out_r[i + k] = r * self.mix;
            }
            i = end;

            // Loop / end handling at sub-chunk granularity.
            let pos = self.wsola.pos();
            if self.loop_b > self.loop_a {
                if pos >= self.loop_b as f64 {
                    self.wsola.reset(self.loop_a as f64);
                }
            } else if pos >= song.frames() as f64 {
                self.playing = false;
                out_l[i..].fill(0.0);
                out_r[i..].fill(0.0);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn song(hz: f32, secs: f32) -> Arc<SongBuffer> {
        let sr = 48_000u32;
        let n = (sr as f32 * secs) as usize;
        let l: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / sr as f32).sin())
            .collect();
        Arc::new(SongBuffer {
            r: l.clone(),
            l,
            sample_rate: sr,
        })
    }

    fn render(p: &mut SongPlayer, frames: usize) -> Vec<f32> {
        let mut l = vec![0.0; frames];
        let mut r = vec![0.0; frames];
        p.render(&mut l, &mut r);
        l
    }

    #[test]
    fn plays_at_pitch_when_neutral() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 3.0));
        p.set_mix(1.0);
        p.play();
        let out = render(&mut p, 48_000);
        let mid = &out[10_000..40_000];
        assert!(goertzel(mid, 48_000.0, 220.0) > 0.3, "plays the tone");
    }

    #[test]
    fn stopped_is_silent() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 2.0));
        p.set_mix(1.0);
        // Not playing yet.
        let out = render(&mut p, 4_800);
        assert_eq!(out.iter().fold(0.0f32, |m, s| m.max(s.abs())), 0.0);
    }

    #[test]
    fn transpose_up_an_octave_doubles_the_pitch() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 3.0));
        p.set_mix(1.0);
        p.set_semitones(12.0); // +1 octave → 440 Hz
        p.play();
        let out = render(&mut p, 48_000);
        let mid = &out[12_000..40_000];
        let f440 = goertzel(mid, 48_000.0, 440.0);
        let f220 = goertzel(mid, 48_000.0, 220.0);
        assert!(f440 > f220 * 2.0, "octave up: 440={f440} vs 220={f220}");
    }

    #[test]
    fn half_speed_keeps_pitch() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 3.0));
        p.set_mix(1.0);
        p.set_speed(0.5);
        p.play();
        let out = render(&mut p, 48_000);
        let mid = &out[12_000..40_000];
        let f220 = goertzel(mid, 48_000.0, 220.0);
        let f110 = goertzel(mid, 48_000.0, 110.0);
        assert!(f220 > 0.2 && f220 > f110 * 3.0, "slow, same pitch: {f220}");
    }

    #[test]
    fn ab_loop_keeps_the_cursor_in_range() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 5.0));
        p.set_mix(1.0);
        p.play();
        p.set_loop(0, 24_000); // loop the first 0.5 s
        // Render 2 s of output at neutral speed — far more than the loop length.
        for _ in 0..20 {
            let _ = render(&mut p, 4_800);
            assert!(
                p.pos_frames() <= 24_000 + 2_000,
                "cursor stayed in the loop: {}",
                p.pos_frames()
            );
        }
    }

    #[test]
    fn stops_at_the_end_without_a_loop() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 0.5)); // short
        p.set_mix(1.0);
        p.play();
        // Render well past the song length.
        for _ in 0..20 {
            let _ = render(&mut p, 4_800);
        }
        assert!(!p.is_playing(), "playback stops at the end");
    }

    #[test]
    fn mix_zero_is_silent_and_bounded() {
        let mut p = SongPlayer::new();
        p.set_song(song(220.0, 2.0));
        p.play();
        p.set_mix(0.0);
        let out = render(&mut p, 9_600);
        assert_eq!(out.iter().fold(0.0f32, |m, s| m.max(s.abs())), 0.0);

        p.set_mix(1.0);
        p.set_semitones(5.0);
        p.set_speed(0.7);
        let out = render(&mut p, 9_600);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() < 2.0));
    }

    #[test]
    fn peaks_downsample_the_waveform() {
        let s = song(220.0, 1.0);
        let peaks = s.peaks(100);
        assert_eq!(peaks.len(), 100);
        assert!(peaks.iter().all(|&p| (0.0..=1.01).contains(&p)));
        assert!(
            peaks.iter().any(|&p| p > 0.5),
            "a full-scale sine has peaks"
        );
    }
}
