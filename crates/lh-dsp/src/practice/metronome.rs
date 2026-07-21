//! Tempo-locked click generator (PRD 019, Phase 1).
//!
//! A [`Metronome`] renders an enveloped sine "click" on every beat, accenting
//! beat 1 of the bar. It is driven entirely by an internal sample clock: the
//! app's player thread pushes the rendered mono click into the engine's aux
//! ring, so the click stays phase-locked to the samples that reach the device
//! without any audio-thread state. The BPM comes from the rig's global tempo
//! (ADR 014); a tempo change just moves the beat threshold — no phase reset.
//!
//! Not an [`Effect`](crate::Effect): the metronome is a *monitor* source summed
//! after the amp chain, never processed by it.

use lh_core::tempo::clamp_bpm;

/// Click voicing. Accent (beat 1) sits a fifth-ish above the plain beat and a
/// touch louder, the classic "high tick / low tock" so the downbeat reads
/// without staring at a light.
const ACCENT_HZ: f32 = 1_500.0;
const BEAT_HZ: f32 = 1_000.0;
const ACCENT_GAIN: f32 = 1.0;
const BEAT_GAIN: f32 = 0.72;
/// Peak amplitude of a full-volume accent click, before the user's level. Kept
/// conservative: the aux bus sums *after* the safety limiter (PRD 019), so the
/// click leans on its own restraint to stay clear of the ceiling.
const MASTER: f32 = 0.6;
/// Click envelope: a short linear attack (de-click the click) into an
/// exponential decay. ~50 ms total — a crisp tick, gone before the next beat.
const ATTACK_MS: f32 = 0.5;
const DECAY_TAU_MS: f32 = 18.0;
/// The decaying tail is cut once it falls below this, freeing the voice.
const CLICK_FLOOR: f32 = 1e-3;

/// Default beats per bar and a sane clamp (1..=16 covers common meters without
/// pretending to parse `7/8` groupings — that is a Phase 2+ concern).
const DEFAULT_BEATS_PER_BAR: u32 = 4;
const MAX_BEATS_PER_BAR: u32 = 16;

/// A tempo-locked click generator. Render mono blocks with [`Self::render`];
/// the caller duplicates to stereo for the aux bus.
pub struct Metronome {
    sample_rate: f32,
    bpm: f32,
    volume: f32,
    beats_per_bar: u32,
    accent: bool,

    /// Samples of one beat at the current tempo (recomputed on `set_bpm`).
    samples_per_beat: u32,
    /// Countdown to the next beat onset; 0 = fire on the next sample.
    to_next_beat: u32,
    /// The beat about to fire (0 = the accented downbeat).
    beat_index: u32,

    /// Active click voice state (envelope + sine phase).
    voice: Option<ClickVoice>,
    /// Per-sample sine-phase increment for the active voice.
    phase_inc: f32,
    /// Per-sample envelope multiplier once the attack finishes.
    decay_coef: f32,
    /// Attack length in samples (linear ramp 0→1).
    attack_len: u32,
}

/// One in-flight click. The sine phase increment lives on the parent
/// ([`Metronome::phase_inc`]); the voice carries only its evolving state.
struct ClickVoice {
    phase: f32,
    /// Running exponential envelope value (after the attack).
    env: f32,
    /// Remaining attack samples; while > 0 the envelope ramps up linearly.
    attack_left: u32,
    gain: f32,
}

impl Default for Metronome {
    fn default() -> Self {
        Self::new()
    }
}

impl Metronome {
    pub fn new() -> Self {
        let mut m = Self {
            sample_rate: 48_000.0,
            bpm: lh_core::tempo::DEFAULT_BPM,
            volume: 0.6,
            beats_per_bar: DEFAULT_BEATS_PER_BAR,
            accent: true,
            samples_per_beat: 24_000,
            to_next_beat: 0,
            beat_index: 0,
            voice: None,
            phase_inc: 0.0,
            decay_coef: 0.0,
            attack_len: 24,
        };
        m.prepare(48_000);
        m
    }

    /// Configure for a sample rate. Recomputes tempo-derived counts and starts
    /// the bar at beat 1 (a click fires on the first rendered sample).
    pub fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1) as f32;
        self.decay_coef = (-1.0 / (DECAY_TAU_MS * 1e-3 * self.sample_rate)).exp();
        self.attack_len = (ATTACK_MS * 1e-3 * self.sample_rate).ceil().max(1.0) as u32;
        self.recompute_beat();
        self.restart();
    }

    fn recompute_beat(&mut self) {
        let spb = self.sample_rate * 60.0 / clamp_bpm(self.bpm);
        self.samples_per_beat = spb.round().max(1.0) as u32;
    }

    /// Set the tempo (clamped to the musical range). Does **not** reset the
    /// running phase — a live tempo change reshapes the next beat, no glitch.
    pub fn set_bpm(&mut self, bpm: f32) {
        let bpm = clamp_bpm(bpm);
        if (bpm - self.bpm).abs() > f32::EPSILON {
            self.bpm = bpm;
            self.recompute_beat();
        }
    }

    /// Click level, `0.0..=1.0`.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Beats per bar (the accent recurs every `n`). Clamped to `1..=16`.
    pub fn set_beats_per_bar(&mut self, n: u32) {
        self.beats_per_bar = n.clamp(1, MAX_BEATS_PER_BAR);
        if self.beat_index >= self.beats_per_bar {
            self.beat_index = 0;
        }
    }

    /// Whether beat 1 is accented (higher/louder). With accent off every beat
    /// is the plain tick.
    pub fn set_accent(&mut self, on: bool) {
        self.accent = on;
    }

    /// Restart the bar: the next rendered sample fires an accented downbeat.
    /// Used on enable and for a count-in lead so the click always begins on 1.
    pub fn restart(&mut self) {
        self.to_next_beat = 0;
        self.beat_index = 0;
        self.voice = None;
    }

    /// Render one mono block of click audio, advancing the beat clock. Silent
    /// between clicks; the caller mixes the result into the (stereo) aux bus.
    pub fn render(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            if self.to_next_beat == 0 {
                self.trigger(self.beat_index);
                self.beat_index = (self.beat_index + 1) % self.beats_per_bar;
                self.to_next_beat = self.samples_per_beat;
            }
            self.to_next_beat -= 1;
            *s = self.tick_voice();
        }
    }

    /// Begin a click for `beat_index` (0 = accented downbeat when enabled).
    fn trigger(&mut self, beat_index: u32) {
        let is_accent = self.accent && beat_index == 0;
        let freq = if is_accent { ACCENT_HZ } else { BEAT_HZ };
        self.phase_inc = std::f32::consts::TAU * freq / self.sample_rate;
        self.voice = Some(ClickVoice {
            phase: 0.0,
            env: 1.0,
            attack_left: self.attack_len,
            gain: if is_accent { ACCENT_GAIN } else { BEAT_GAIN },
        });
    }

    /// Advance the active click one sample; returns its output (0 when idle).
    fn tick_voice(&mut self) -> f32 {
        let Some(v) = self.voice.as_mut() else {
            return 0.0;
        };
        // Envelope: linear attack, then exponential decay to the floor.
        let amp = if v.attack_left > 0 {
            let g = 1.0 - v.attack_left as f32 / self.attack_len as f32;
            v.attack_left -= 1;
            g
        } else {
            let g = v.env;
            v.env *= self.decay_coef;
            g
        };
        let s = v.phase.sin() * amp * v.gain * self.volume * MASTER;
        v.phase += self.phase_inc;
        if v.phase >= std::f32::consts::TAU {
            v.phase -= std::f32::consts::TAU;
        }
        // Retire a spent voice so the block goes truly silent between clicks.
        if v.attack_left == 0 && v.env < CLICK_FLOOR {
            self.voice = None;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Onset sample indices: a loud sample after a real gap of silence. The
    /// click is a decaying sine, so it dips near zero every half-cycle — a
    /// bare "rose above threshold" test would fire on every crossing. Requiring
    /// a `GAP`-long silent run first isolates true beat onsets (clicks are
    /// milliseconds apart at most; beats are tens of ms apart at least).
    fn onsets(block: &[f32]) -> Vec<usize> {
        const GAP: usize = 256;
        let mut out = Vec::new();
        let mut silent_run = GAP; // count an onset at index 0
        for (i, &s) in block.iter().enumerate() {
            if s.abs() > 1e-3 {
                if silent_run >= GAP {
                    out.push(i);
                }
                silent_run = 0;
            } else {
                silent_run += 1;
            }
        }
        out
    }

    /// Peak magnitude in `[start, start+win)`.
    fn peak(block: &[f32], start: usize, win: usize) -> f32 {
        block[start..(start + win).min(block.len())]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn clicks_land_on_the_beat() {
        let mut m = Metronome::new();
        m.prepare(48_000);
        m.set_bpm(120.0); // 120 bpm @ 48k → 24000 samples/beat
        let mut buf = vec![0.0; 24_000 * 4];
        m.render(&mut buf);
        let onsets = onsets(&buf);
        // Four beats over four beat-lengths, at ~0, 24000, 48000, 72000. The
        // soft attack (and sin(0)=0) delays the audible onset a few samples.
        assert_eq!(onsets.len(), 4, "onsets: {onsets:?}");
        for (k, &o) in onsets.iter().enumerate() {
            let expected = 24_000 * k;
            assert!(
                (o as i64 - expected as i64).abs() <= 8,
                "beat {k} at {o}, expected ~{expected}"
            );
        }
    }

    #[test]
    fn beat_one_is_accented_and_recurs_per_bar() {
        let mut m = Metronome::new();
        m.prepare(48_000);
        m.set_bpm(120.0);
        m.set_beats_per_bar(4);
        let spb = 24_000;
        let mut buf = vec![0.0; spb * 8];
        m.render(&mut buf);
        let accent0 = peak(&buf, 0, 512);
        let beat1 = peak(&buf, spb, 512);
        let accent4 = peak(&buf, spb * 4, 512);
        let beat5 = peak(&buf, spb * 5, 512);
        assert!(accent0 > beat1 * 1.2, "downbeat should be louder");
        // The accent recurs every 4 beats.
        assert!(
            (accent4 - accent0).abs() < accent0 * 0.05,
            "beat 5 re-accents"
        );
        assert!(
            (beat5 - beat1).abs() < beat1 * 0.05,
            "beat 6 is a plain tick"
        );
    }

    #[test]
    fn accent_off_makes_every_beat_equal() {
        let mut m = Metronome::new();
        m.prepare(48_000);
        m.set_bpm(120.0);
        m.set_accent(false);
        let spb = 24_000;
        let mut buf = vec![0.0; spb * 3];
        m.render(&mut buf);
        let a = peak(&buf, 0, 512);
        let b = peak(&buf, spb, 512);
        assert!((a - b).abs() < a * 0.05, "no accent: {a} vs {b}");
    }

    #[test]
    fn volume_scales_the_click() {
        let mut m = Metronome::new();
        m.prepare(48_000);
        m.set_bpm(120.0);
        m.set_volume(1.0);
        let mut loud = vec![0.0; 4_800];
        m.render(&mut loud);
        m.restart();
        m.set_volume(0.5);
        let mut soft = vec![0.0; 4_800];
        m.render(&mut soft);
        let pl = peak(&loud, 0, 512);
        let ps = peak(&soft, 0, 512);
        assert!((pl - 2.0 * ps).abs() < pl * 0.05, "half volume ≈ half peak");
        // Full-scale accent stays clear of the ceiling.
        assert!(pl < 0.7, "conservative peak, got {pl}");

        m.set_volume(0.0);
        m.restart();
        let mut silent = vec![0.0; 4_800];
        m.render(&mut silent);
        assert_eq!(peak(&silent, 0, 4_800), 0.0, "volume 0 = silence");
    }

    #[test]
    fn restart_fires_the_downbeat_immediately() {
        let mut m = Metronome::new();
        m.prepare(48_000);
        m.set_bpm(90.0);
        // Render partway into a bar, then restart mid-beat.
        let mut warm = vec![0.0; 10_000];
        m.render(&mut warm);
        m.restart();
        let mut buf = vec![0.0; 4_800];
        m.render(&mut buf);
        let onsets = onsets(&buf);
        assert!(
            onsets.first().is_some_and(|&o| o <= 8),
            "count-in starts on 1: {onsets:?}"
        );
    }

    #[test]
    fn tempo_change_tracks_the_new_beat_spacing() {
        let mut m = Metronome::new();
        m.prepare(48_000);
        m.set_bpm(60.0); // 48000 samples/beat
        let mut buf = vec![0.0; 48_000 * 2];
        m.render(&mut buf);
        assert_eq!(onsets(&buf).len(), 2, "two beats at 60 bpm over 2 s");

        m.restart();
        m.set_bpm(240.0); // 12000 samples/beat
        let mut fast = vec![0.0; 48_000];
        m.render(&mut fast);
        assert_eq!(onsets(&fast).len(), 4, "four beats at 240 bpm over 1 s");
    }

    #[test]
    fn output_is_finite_and_bounded_across_rates() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut m = Metronome::new();
            m.prepare(sr);
            m.set_bpm(137.0);
            m.set_volume(1.0);
            let mut buf = vec![0.0; sr as usize];
            m.render(&mut buf);
            assert!(buf.iter().all(|s| s.is_finite()), "finite @ {sr}");
            assert!(buf.iter().all(|s| s.abs() <= 1.0), "bounded @ {sr}");
        }
    }
}
