//! Procedural drum-groove generator (PRD 019, Phase 2).
//!
//! A [`DrumMachine`] synthesises a looping drum pattern **at the exact global
//! tempo** (ADR 021). Building the beat from scratch at the target BPM tightens
//! the tempo lock a stretched loop can only approximate, and ships no binary
//! samples — the built-in kit is a small analog-style synth (kick / snare /
//! closed + open hi-hat / tom). It renders a mono block the app's player thread
//! sums into the aux bus alongside the [`Metronome`](super::Metronome).
//!
//! Deterministic: the noise voices run a seeded xorshift PRNG, so a rendered
//! bar is reproducible (the tests lean on this). Not an [`Effect`](crate::Effect)
//! — like the metronome it is a monitor source, summed after the amp chain.

use lh_core::tempo::clamp_bpm;

/// Sixteenth-note steps per bar (one bar of 4/4). Patterns are one bar long.
const STEPS: usize = 16;
/// Peak-ish amplitude scaling so a full-velocity downbeat (kick + hat) lands
/// well under the ceiling — the groove sums into the aux *after* the safety
/// limiter (PRD 019), so it stays polite by construction.
const MASTER: f32 = 0.5;

/// A one-bar pattern: per-voice velocities on the 16-step grid (0 = no hit).
struct Pattern {
    name: &'static str,
    kick: [f32; STEPS],
    snare: [f32; STEPS],
    chat: [f32; STEPS],
    ohat: [f32; STEPS],
    tom: [f32; STEPS],
}

/// The built-in grooves, in menu order. Append-only (indices are the API).
static PATTERNS: &[Pattern] = &[
    // Rock: kick on 1 & 3 (+ the "and" of 3), backbeat snare, straight 8th hats.
    Pattern {
        name: "rock",
        kick: [
            1.0, 0., 0., 0., 0., 0., 0., 0., 0.9, 0., 0.6, 0., 0., 0., 0., 0.,
        ],
        snare: [
            0., 0., 0., 0., 0.95, 0., 0., 0., 0., 0., 0., 0., 0.95, 0., 0., 0.,
        ],
        chat: [
            0.7, 0., 0.5, 0., 0.7, 0., 0.5, 0., 0.7, 0., 0.5, 0., 0.7, 0., 0.5, 0.,
        ],
        ohat: [0.; STEPS],
        tom: [0.; STEPS],
    },
    // Funk: syncopated kick, ghost-note snare, busy 16th hats.
    Pattern {
        name: "funk",
        kick: [
            1.0, 0., 0., 0.7, 0., 0., 0.8, 0., 0., 0., 0.7, 0., 0., 0., 0., 0.,
        ],
        snare: [
            0., 0., 0., 0., 0.95, 0., 0., 0.4, 0., 0., 0., 0., 0.95, 0., 0., 0.45,
        ],
        chat: [
            0.65, 0.4, 0.5, 0.4, 0.65, 0.4, 0.5, 0.4, 0.65, 0.4, 0.5, 0.4, 0.65, 0.4, 0.5, 0.4,
        ],
        ohat: [0.; STEPS],
        tom: [0.; STEPS],
    },
    // Metal: driving 8th-note kick, hard backbeat, 8th hats.
    Pattern {
        name: "metal",
        kick: [
            0.95, 0., 0.85, 0., 0.9, 0., 0.85, 0., 0.95, 0., 0.85, 0., 0.9, 0., 0.85, 0.,
        ],
        snare: [
            0., 0., 0., 0., 1.0, 0., 0., 0., 0., 0., 0., 0., 1.0, 0., 0., 0.,
        ],
        chat: [
            0.65, 0., 0.55, 0., 0.65, 0., 0.55, 0., 0.65, 0., 0.55, 0., 0.65, 0., 0.55, 0.,
        ],
        ohat: [0.; STEPS],
        tom: [0.; STEPS],
    },
    // Ballad: half-time feel — sparse kick, backbeat on beat 3, soft 8th hats.
    Pattern {
        name: "ballad",
        kick: [
            1.0, 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.7, 0., 0., 0., 0., 0.,
        ],
        snare: [
            0., 0., 0., 0., 0., 0., 0., 0., 0.9, 0., 0., 0., 0., 0., 0., 0.,
        ],
        chat: [
            0.4, 0., 0.35, 0., 0.4, 0., 0.35, 0., 0.4, 0., 0.35, 0., 0.4, 0., 0.35, 0.,
        ],
        ohat: [
            0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.5, 0.,
        ],
        tom: [0.; STEPS],
    },
];

/// A one-bar fill: kick pickup, then a rising tom roll into the next downbeat.
static FILL: Pattern = Pattern {
    name: "fill",
    kick: [
        1.0, 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.,
    ],
    snare: [
        0., 0., 0., 0., 0.8, 0., 0., 0., 0.7, 0., 0., 0., 0., 0., 0., 0.,
    ],
    chat: [
        0.6, 0., 0., 0., 0.6, 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.,
    ],
    ohat: [0.; STEPS],
    tom: [
        0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.7, 0.75, 0.8, 0.85, 0.9, 0.95,
    ],
};

/// Number of built-in patterns.
pub fn pattern_count() -> usize {
    PATTERNS.len()
}

/// Menu name of a pattern index (`"?"` out of range).
pub fn pattern_name(index: usize) -> &'static str {
    PATTERNS.get(index).map_or("?", |p| p.name)
}

/// Resolve a pattern name (case-insensitive) to its index.
pub fn pattern_index(name: &str) -> Option<usize> {
    let name = name.trim().to_ascii_lowercase();
    PATTERNS.iter().position(|p| p.name == name)
}

/// One synth drum voice. A single instance per kit piece, retriggered on each
/// hit (a new hit restarts the envelope — drums choke themselves, which reads
/// fine). Synthesis is selected by [`Kind`]; unused terms (tone/noise/pitch)
/// carry zero gain, so one `tick` covers every piece.
struct Voice {
    kind: Kind,
    sr: f32,
    active: bool,
    /// Amplitude envelope (starts at the hit velocity, decays exponentially).
    amp: f32,
    amp_coef: f32,
    /// Tone oscillator (kick/snare/tom); `tone_gain` 0 for the hats.
    phase: f32,
    pitch: f32,
    pitch_start: f32,
    pitch_end: f32,
    pitch_coef: f32,
    tone_gain: f32,
    /// Filtered-noise part (snare/hats); `noise_gain` 0 for kick/tom. `lp` is a
    /// one-pole low-pass whose output is subtracted for a high-pass.
    noise_gain: f32,
    hp_k: f32,
    lp: f32,
    gain: f32,
    /// Attack click samples (kick only): a couple ms of noise for the beater.
    click_left: u32,
    click_len: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Kick,
    Snare,
    ClosedHat,
    OpenHat,
    Tom,
}

impl Voice {
    fn new(kind: Kind) -> Self {
        let mut v = Self {
            kind,
            sr: 48_000.0,
            active: false,
            amp: 0.0,
            amp_coef: 0.0,
            phase: 0.0,
            pitch: 0.0,
            pitch_start: 0.0,
            pitch_end: 0.0,
            pitch_coef: 0.0,
            tone_gain: 0.0,
            noise_gain: 0.0,
            hp_k: 0.0,
            lp: 0.0,
            gain: 1.0,
            click_left: 0,
            click_len: 0,
        };
        v.prepare(48_000);
        v
    }

    fn prepare(&mut self, sr: u32) {
        self.sr = sr.max(1) as f32;
        let amp_tau = |ms: f32| (-1.0 / (ms * 1e-3 * self.sr)).exp();
        let pitch_coef = |ms: f32| (-1.0 / (ms * 1e-3 * self.sr)).exp();
        // 1-pole high-pass corner as a low-pass leak coefficient.
        let hp = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / self.sr).exp();
        match self.kind {
            Kind::Kick => {
                // Punchy, not boomy: a fast pitch drop and a tight decay so
                // consecutive kicks don't smear into each other.
                self.amp_coef = amp_tau(32.0);
                self.pitch_start = 150.0;
                self.pitch_end = 48.0;
                self.pitch_coef = pitch_coef(22.0);
                self.tone_gain = 1.0;
                self.noise_gain = 0.0;
                self.gain = 1.0;
                self.click_len = (0.002 * self.sr) as u32;
            }
            Kind::Snare => {
                self.amp_coef = amp_tau(55.0);
                self.pitch_start = 185.0;
                self.pitch_end = 185.0;
                self.pitch_coef = 0.0;
                self.tone_gain = 0.45;
                self.noise_gain = 0.9;
                self.hp_k = hp(320.0);
                self.gain = 0.85;
            }
            Kind::ClosedHat => {
                self.amp_coef = amp_tau(18.0);
                self.tone_gain = 0.0;
                self.noise_gain = 1.0;
                self.hp_k = hp(7_000.0);
                self.gain = 0.32;
            }
            Kind::OpenHat => {
                self.amp_coef = amp_tau(80.0);
                self.tone_gain = 0.0;
                self.noise_gain = 1.0;
                self.hp_k = hp(7_000.0);
                self.gain = 0.34;
            }
            Kind::Tom => {
                self.amp_coef = amp_tau(70.0);
                self.pitch_start = 180.0;
                self.pitch_end = 110.0;
                self.pitch_coef = pitch_coef(45.0);
                self.tone_gain = 1.0;
                self.noise_gain = 0.0;
                self.gain = 0.7;
            }
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.active = false;
        self.amp = 0.0;
        self.phase = 0.0;
        self.pitch = self.pitch_start;
        self.lp = 0.0;
        self.click_left = 0;
    }

    /// Strike the voice at `velocity` (0..1) — restarts the envelope.
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amp = velocity;
        self.phase = 0.0;
        self.pitch = self.pitch_start;
        self.click_left = self.click_len;
    }

    /// One sample of this voice; `noise` is a fresh white-noise draw (used by
    /// the noise voices and the kick's beater click).
    fn tick(&mut self, noise: f32) -> f32 {
        if !self.active {
            return 0.0;
        }
        let tone = if self.tone_gain > 0.0 {
            let s = self.phase.sin();
            self.phase += std::f32::consts::TAU * self.pitch / self.sr;
            if self.phase >= std::f32::consts::TAU {
                self.phase -= std::f32::consts::TAU;
            }
            if self.pitch_coef > 0.0 {
                self.pitch = self.pitch_end + (self.pitch - self.pitch_end) * self.pitch_coef;
            }
            s * self.tone_gain
        } else {
            0.0
        };
        let noisy = if self.noise_gain > 0.0 {
            self.lp += (noise - self.lp) * self.hp_k;
            (noise - self.lp) * self.noise_gain
        } else {
            0.0
        };
        let click = if self.click_left > 0 {
            self.click_left -= 1;
            noise * 0.6
        } else {
            0.0
        };
        let out = (tone + noisy + click) * self.amp * self.gain;
        self.amp *= self.amp_coef;
        if self.amp < 1e-4 {
            self.active = false;
        }
        out
    }
}

/// A tempo-locked procedural drum machine. Render mono blocks with
/// [`Self::render`]; the caller sums the result into the aux bus.
pub struct DrumMachine {
    sr: f32,
    bpm: f32,
    volume: f32,
    pattern: usize,

    /// Samples per 16th-note step at the current tempo.
    samples_per_step: u32,
    to_next_step: u32,
    step: usize,

    /// A fill is armed by `fill()`, plays on the next downbeat for one bar.
    fill_pending: bool,
    filling: bool,

    kick: Voice,
    snare: Voice,
    chat: Voice,
    ohat: Voice,
    tom: Voice,

    /// Deterministic white-noise source (xorshift32).
    rng: u32,
}

impl Default for DrumMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl DrumMachine {
    pub fn new() -> Self {
        let mut m = Self {
            sr: 48_000.0,
            bpm: lh_core::tempo::DEFAULT_BPM,
            volume: 0.7,
            pattern: 0,
            samples_per_step: 6_000,
            to_next_step: 0,
            step: 0,
            fill_pending: false,
            filling: false,
            kick: Voice::new(Kind::Kick),
            snare: Voice::new(Kind::Snare),
            chat: Voice::new(Kind::ClosedHat),
            ohat: Voice::new(Kind::OpenHat),
            tom: Voice::new(Kind::Tom),
            rng: 0x9E37_79B9,
        };
        m.prepare(48_000);
        m
    }

    pub fn prepare(&mut self, sample_rate: u32) {
        self.sr = sample_rate.max(1) as f32;
        for v in self.voices_mut() {
            v.prepare(sample_rate);
        }
        self.recompute_step();
        self.restart();
    }

    fn voices_mut(&mut self) -> [&mut Voice; 5] {
        [
            &mut self.kick,
            &mut self.snare,
            &mut self.chat,
            &mut self.ohat,
            &mut self.tom,
        ]
    }

    fn recompute_step(&mut self) {
        // One 16th note = a quarter of a beat.
        let per_beat = self.sr * 60.0 / clamp_bpm(self.bpm);
        self.samples_per_step = (per_beat / 4.0).round().max(1.0) as u32;
    }

    /// Set the tempo (clamped). Keeps the running phase — a live tempo change
    /// reshapes the next step, no glitch.
    pub fn set_bpm(&mut self, bpm: f32) {
        let bpm = clamp_bpm(bpm);
        if (bpm - self.bpm).abs() > f32::EPSILON {
            self.bpm = bpm;
            self.recompute_step();
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Select a built-in pattern by index (clamped).
    pub fn set_pattern(&mut self, index: usize) {
        self.pattern = index.min(PATTERNS.len().saturating_sub(1));
    }

    pub fn pattern(&self) -> usize {
        self.pattern
    }

    /// Arm a one-bar fill — it plays from the next downbeat.
    pub fn fill(&mut self) {
        self.fill_pending = true;
    }

    /// Restart the loop on the next rendered sample (a fresh downbeat). Used on
    /// enable so the groove always starts at step 1.
    pub fn restart(&mut self) {
        self.to_next_step = 0;
        self.step = 0;
        self.filling = false;
        self.fill_pending = false;
        for v in self.voices_mut() {
            v.reset();
        }
    }

    fn noise(&mut self) -> f32 {
        // xorshift32 → [-1, 1)
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Trigger all voices whose current pattern hits `step`.
    fn strike(&mut self, step: usize) {
        let pat = if self.filling {
            &FILL
        } else {
            &PATTERNS[self.pattern]
        };
        let (k, s, c, o, t) = (
            pat.kick[step],
            pat.snare[step],
            pat.chat[step],
            pat.ohat[step],
            pat.tom[step],
        );
        if k > 0.0 {
            self.kick.trigger(k);
        }
        if s > 0.0 {
            self.snare.trigger(s);
        }
        if c > 0.0 {
            self.chat.trigger(c);
        }
        if o > 0.0 {
            self.ohat.trigger(o);
        }
        if t > 0.0 {
            self.tom.trigger(t);
        }
    }

    /// Render one mono block, advancing the step clock.
    pub fn render(&mut self, out: &mut [f32]) {
        for sample in out.iter_mut() {
            if self.to_next_step == 0 {
                // At each downbeat, swap in/out the fill for the coming bar.
                if self.step == 0 {
                    if self.fill_pending {
                        self.filling = true;
                        self.fill_pending = false;
                    } else if self.filling {
                        self.filling = false;
                    }
                }
                self.strike(self.step);
                self.step = (self.step + 1) % STEPS;
                self.to_next_step = self.samples_per_step;
            }
            self.to_next_step -= 1;

            let n = [
                self.noise(),
                self.noise(),
                self.noise(),
                self.noise(),
                self.noise(),
            ];
            let mix = self.kick.tick(n[0])
                + self.snare.tick(n[1])
                + self.chat.tick(n[2])
                + self.ohat.tick(n[3])
                + self.tom.tick(n[4]);
            *sample = mix * self.volume * MASTER;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Onset sample indices: a loud sample after a `GAP`-long silent run.
    fn onsets(block: &[f32], gap: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut silent = gap;
        for (i, &s) in block.iter().enumerate() {
            if s.abs() > 5e-3 {
                if silent >= gap {
                    out.push(i);
                }
                silent = 0;
            } else {
                silent += 1;
            }
        }
        out
    }

    fn peak(block: &[f32]) -> f32 {
        block.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Total signal energy — a hit-density proxy robust to overlapping tails.
    fn energy(block: &[f32]) -> f32 {
        block.iter().map(|s| s * s).sum()
    }

    /// Render `secs` of a named pattern at `bpm` into a fresh machine.
    fn render(name: &str, bpm: f32, secs: f32) -> Vec<f32> {
        let mut m = DrumMachine::new();
        m.prepare(SR);
        m.set_pattern(pattern_index(name).unwrap());
        m.set_bpm(bpm);
        let mut buf = vec![0.0; (SR as f32 * secs) as usize];
        m.render(&mut buf);
        buf
    }

    const SR: u32 = 48_000;

    #[test]
    fn steps_land_on_the_sixteenth_grid() {
        // Metal at 120 bpm: hits on every even step (8th notes). At 48k a step
        // is 6000 samples, so onsets land at multiples of 12000.
        let mut m = DrumMachine::new();
        m.prepare(SR);
        m.set_pattern(pattern_index("metal").unwrap());
        m.set_bpm(120.0);
        let bar = 96_000; // 4 beats
        let mut buf = vec![0.0; bar];
        m.render(&mut buf);
        let onsets = onsets(&buf, 3_000);
        assert!(onsets.len() >= 6, "metal bar is busy: {onsets:?}");
        for &o in &onsets {
            let nearest = ((o as f32 / 12_000.0).round() * 12_000.0) as i64;
            assert!(
                (o as i64 - nearest).abs() <= 200,
                "onset {o} off the 8th grid (nearest {nearest})"
            );
        }
    }

    #[test]
    fn patterns_differ() {
        // Ballad is sparse; metal drives every 8th — clearly more energy per
        // bar (energy is robust to overlapping tails, unlike onset counts).
        let metal = energy(&render("metal", 120.0, 2.0));
        let ballad = energy(&render("ballad", 120.0, 2.0));
        assert!(metal > ballad * 1.5, "metal {metal} vs ballad {ballad}");
    }

    #[test]
    fn tempo_change_scales_the_bar() {
        // Same window, same pattern: a faster tempo packs more bars — more hits
        // — so more energy.
        let fast = energy(&render("rock", 180.0, 2.0));
        let slow = energy(&render("rock", 90.0, 2.0));
        assert!(fast > slow * 1.3, "180 bpm {fast} vs 90 bpm {slow}");
    }

    #[test]
    fn volume_scales_and_zero_is_silent() {
        let mut m = DrumMachine::new();
        m.prepare(SR);
        m.set_bpm(120.0);
        m.set_volume(1.0);
        let mut loud = vec![0.0; 96_000];
        m.render(&mut loud);

        m.restart();
        m.set_volume(0.0);
        let mut silent = vec![0.0; 96_000];
        m.render(&mut silent);
        assert_eq!(peak(&silent), 0.0, "volume 0 = silence");
        assert!(peak(&loud) > 0.1, "audible at full volume");
        assert!(
            peak(&loud) <= 1.0,
            "bounded below full scale: {}",
            peak(&loud)
        );
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut m = DrumMachine::new();
            m.prepare(SR);
            m.set_bpm(128.0);
            let mut buf = vec![0.0; 48_000];
            m.render(&mut buf);
            buf
        };
        assert_eq!(run(), run(), "seeded synthesis must be reproducible");
    }

    #[test]
    fn fill_adds_hits() {
        // Compare like-for-like: one ballad bar with vs without a fill armed.
        let bar = 96_000; // one bar at 120 bpm
        let filled = {
            let mut m = DrumMachine::new();
            m.prepare(SR);
            m.set_pattern(pattern_index("ballad").unwrap());
            m.set_bpm(120.0);
            m.fill(); // plays on the coming downbeat
            let mut buf = vec![0.0; bar];
            m.render(&mut buf);
            energy(&buf)
        };
        let plain = energy(&render("ballad", 120.0, 2.0)[..bar]);
        assert!(filled > plain, "fill {filled} vs ballad bar {plain}");
    }

    #[test]
    fn finite_and_bounded_across_rates() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut m = DrumMachine::new();
            m.prepare(sr);
            m.set_pattern(pattern_index("funk").unwrap());
            m.set_bpm(150.0);
            m.set_volume(1.0);
            let mut buf = vec![0.0; sr as usize];
            m.render(&mut buf);
            assert!(buf.iter().all(|s| s.is_finite()), "finite @ {sr}");
            assert!(buf.iter().all(|s| s.abs() <= 1.0), "bounded @ {sr}");
        }
    }
}
