//! Monophonic pitch detection for the tuner.
//!
//! YIN (de Cheveigné & Kawahara 2002): a difference function over lag τ,
//! cumulative-mean normalized so the fundamental wins over its harmonics,
//! absolute threshold + parabolic interpolation for sub-sample precision.
//!
//! This is an *analyzer*, not an [`Effect`](crate::Effect): it runs on a
//! control/UI thread over samples tapped from the audio input — never on the
//! audio thread. All buffers are allocated in [`Tuner::new`]; `feed` and
//! `estimate` allocate nothing, so a control thread can call them freely.

use lh_core::lin_to_db;

/// Lowest detectable fundamental (covers drop-A 7-string, 55 Hz).
pub const MIN_FREQ: f32 = 55.0;
/// Highest detectable fundamental (frets high on the neck, harmonics tuning).
pub const MAX_FREQ: f32 = 1_500.0;
/// Below this input level the tuner reports silence.
const GATE_DB: f32 = -50.0;
/// YIN absolute threshold: first dip under this wins…
const THRESHOLD: f32 = 0.10;
/// …and if nothing dips under this, the input is unpitched (noise, chords).
const MAX_DIP: f32 = 0.15;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// A detected pitch with equal-temperament context (A4 = 440 Hz).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    pub freq_hz: f32,
    /// Fractional MIDI note number (69.0 = A4).
    pub midi: f32,
}

impl PitchEstimate {
    fn from_freq(freq_hz: f32) -> Self {
        Self {
            freq_hz,
            midi: 69.0 + 12.0 * (freq_hz / 440.0).log2(),
        }
    }

    /// Nearest tempered note, e.g. `"E"` for 82.4 Hz.
    pub fn note_name(&self) -> &'static str {
        let nearest = self.midi.round() as i32;
        NOTE_NAMES[nearest.rem_euclid(12) as usize]
    }

    /// Octave of the nearest note in scientific pitch notation (E2 = low E).
    pub fn octave(&self) -> i32 {
        (self.midi.round() as i32) / 12 - 1
    }

    /// Deviation from the nearest tempered note in cents, −50‥50.
    pub fn cents(&self) -> f32 {
        (self.midi - self.midi.round()) * 100.0
    }
}

/// Sliding-window YIN detector. `feed` audio in, `estimate` at display rate.
pub struct Tuner {
    sample_rate: f32,
    /// Newest `buf.len()` samples, oldest first once `filled`.
    buf: Vec<f32>,
    write: usize,
    filled: usize,
    /// Analysis scratch: the window laid out in time order.
    window: Vec<f32>,
    /// Cumulative-mean-normalized difference per lag.
    diff: Vec<f32>,
    tau_min: usize,
    tau_max: usize,
}

impl Tuner {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let tau_max = (sr / MIN_FREQ).ceil() as usize;
        let tau_min = (sr / MAX_FREQ).floor().max(2.0) as usize;
        // d(τ) integrates over tau_max terms, so the window holds 2·tau_max.
        let len = 2 * tau_max;
        Self {
            sample_rate: sr,
            buf: vec![0.0; len],
            write: 0,
            filled: 0,
            window: vec![0.0; len],
            diff: vec![0.0; tau_max + 1],
            tau_min,
            tau_max,
        }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.filled = 0;
    }

    /// Append tapped input samples (any block size).
    pub fn feed(&mut self, samples: &[f32]) {
        for &s in samples {
            self.buf[self.write] = s;
            self.write = (self.write + 1) % self.buf.len();
        }
        self.filled = (self.filled + samples.len()).min(self.buf.len());
    }

    /// Analyze the current window. `None` until enough signal has arrived,
    /// on silence, or when the input has no single clear pitch.
    pub fn estimate(&mut self) -> Option<PitchEstimate> {
        let len = self.buf.len();
        if self.filled < len {
            return None;
        }

        // Unroll the ring into time order: oldest sample first.
        let (tail, head) = self.buf.split_at(self.write);
        self.window[..head.len()].copy_from_slice(head);
        self.window[head.len()..].copy_from_slice(tail);

        let rms = (self.window.iter().map(|x| x * x).sum::<f32>() / len as f32).sqrt();
        if lin_to_db(rms.max(1e-12)) < GATE_DB {
            return None;
        }

        // Difference function d(τ) with a fixed integration window,
        // normalized to d'(τ) on the fly (cumulative mean).
        let w = self.tau_max;
        self.diff[0] = 1.0;
        let mut running_sum = 0.0;
        for tau in 1..=self.tau_max {
            let mut d = 0.0;
            for j in 0..w {
                let delta = self.window[j] - self.window[j + tau];
                d += delta * delta;
            }
            running_sum += d;
            self.diff[tau] = if running_sum > 0.0 {
                d * tau as f32 / running_sum
            } else {
                1.0
            };
        }

        // First dip under THRESHOLD, extended to its local minimum.
        let mut tau = None;
        for t in self.tau_min..=self.tau_max {
            if self.diff[t] < THRESHOLD {
                let mut best = t;
                while best < self.tau_max && self.diff[best + 1] < self.diff[best] {
                    best += 1;
                }
                tau = Some(best);
                break;
            }
        }
        // Fallback: global minimum, if convincing enough.
        let tau = tau.or_else(|| {
            let (best, &dip) = self.diff[self.tau_min..=self.tau_max]
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))?;
            (dip < MAX_DIP).then_some(best + self.tau_min)
        })?;

        // Parabolic interpolation for sub-sample lag.
        let refined = if tau > self.tau_min && tau < self.tau_max {
            let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-12 {
                tau as f32 + 0.5 * (a - c) / denom
            } else {
                tau as f32
            }
        } else {
            tau as f32
        };

        let freq = self.sample_rate / refined;
        (MIN_FREQ..=MAX_FREQ)
            .contains(&freq)
            .then(|| PitchEstimate::from_freq(freq))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard-tuning open strings, E2..E4.
    const OPEN_STRINGS: [(f32, &str, i32); 6] = [
        (82.407, "E", 2),
        (110.0, "A", 2),
        (146.832, "D", 3),
        (195.998, "G", 3),
        (246.942, "B", 3),
        (329.628, "E", 4),
    ];

    fn feed_sine(tuner: &mut Tuner, sr: f32, freq: f32, amp: f32, secs: f32) {
        let n = (sr * secs) as usize;
        let block: Vec<f32> = (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / sr).sin())
            .collect();
        tuner.feed(&block);
    }

    #[test]
    fn detects_open_strings_within_two_cents() {
        for sr in [44_100u32, 48_000, 96_000] {
            for (freq, name, octave) in OPEN_STRINGS {
                let mut tuner = Tuner::new(sr);
                feed_sine(&mut tuner, sr as f32, freq, 0.3, 0.2);
                let est = tuner
                    .estimate()
                    .unwrap_or_else(|| panic!("no pitch for {freq} Hz @ {sr}"));
                assert_eq!(est.note_name(), name, "{freq} Hz @ {sr}");
                assert_eq!(est.octave(), octave, "{freq} Hz @ {sr}");
                assert!(
                    est.cents().abs() < 2.0,
                    "{freq} Hz @ {sr}: off by {:.2} cents",
                    est.cents()
                );
            }
        }
    }

    #[test]
    fn no_octave_error_on_harmonic_rich_pluck() {
        // 2nd harmonic louder than the fundamental — a plucked low E does this.
        let sr = 48_000u32;
        let mut tuner = Tuner::new(sr);
        let f0 = 82.407f32;
        let n = (sr as f32 * 0.2) as usize;
        let block: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let ph = std::f32::consts::TAU * f0 * t;
                0.2 * ph.sin() + 0.35 * (2.0 * ph).sin() + 0.15 * (3.0 * ph + 0.4).sin()
            })
            .collect();
        tuner.feed(&block);
        let est = tuner.estimate().expect("pitch");
        assert_eq!((est.note_name(), est.octave()), ("E", 2));
        assert!(est.cents().abs() < 3.0, "off by {:.2} cents", est.cents());
    }

    #[test]
    fn reports_flat_string_in_cents() {
        // E2 tuned ~30 cents flat.
        let sr = 48_000u32;
        let mut tuner = Tuner::new(sr);
        feed_sine(&mut tuner, sr as f32, 81.0, 0.3, 0.2);
        let est = tuner.estimate().expect("pitch");
        assert_eq!((est.note_name(), est.octave()), ("E", 2));
        assert!(
            (-32.0..=-28.0).contains(&est.cents()),
            "expected ≈ -29.8 cents, got {:.2}",
            est.cents()
        );
    }

    #[test]
    fn silence_and_noise_report_nothing() {
        let sr = 48_000u32;
        let mut tuner = Tuner::new(sr);
        tuner.feed(&vec![0.0; 8_192]);
        assert_eq!(tuner.estimate(), None, "silence");

        // Deterministic white-ish noise (LCG), no stable pitch.
        let mut state = 0x1234_5678u32;
        let noise: Vec<f32> = (0..8_192)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1 << 24) as f32 - 0.5
            })
            .collect();
        tuner.feed(&noise);
        assert_eq!(tuner.estimate(), None, "noise");
    }

    #[test]
    fn needs_a_full_window_before_estimating() {
        let mut tuner = Tuner::new(48_000);
        feed_sine(&mut tuner, 48_000.0, 110.0, 0.3, 0.01); // 480 samples ≪ window
        assert_eq!(tuner.estimate(), None);
    }

    #[test]
    fn estimate_is_stable_across_feed_chunk_sizes() {
        let sr = 48_000u32;
        for chunk in [32usize, 64, 483, 1024] {
            let mut tuner = Tuner::new(sr);
            let n = (sr as f32 * 0.2) as usize;
            let samples: Vec<f32> = (0..n)
                .map(|i| 0.3 * (std::f32::consts::TAU * 110.0 * i as f32 / sr as f32).sin())
                .collect();
            for block in samples.chunks(chunk) {
                tuner.feed(block);
            }
            let est = tuner.estimate().expect("pitch");
            assert!(
                (est.freq_hz - 110.0).abs() < 0.2,
                "chunk {chunk}: {} Hz",
                est.freq_hz
            );
        }
    }
}
