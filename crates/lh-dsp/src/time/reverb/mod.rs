//! Reverb: a family of twelve machines behind one chain slot (PRD 005) —
//! hall, room, plate, spring, swell, bloom, cloud, chorale, shimmer,
//! magneto, nonlinear, reflections — inspired by the classic studio/pedal
//! archetypes (the BigSky machine list), all built on one engine.
//!
//! The core is still the M5 8-line Householder FDN (`H = I − (2/N)·J`,
//! orthogonal, O(N)): per-line gain `g = 10^(−3·delay/t60)` makes every path
//! lose 60 dB in exactly `t60` seconds, unconditionally stable. What the
//! family adds, per [`VoiceDef`] constants (like [`super::delay`]):
//!
//! - **Interpolated tank lines**: line lengths scale with the voice's `size`
//!   and wobble under the `mod` knob (one LFO distributed across lines via
//!   phase rotation — 1 sin + 1 cos per sample, not 8).
//! - **Structural kinds**: most voices are `Tank`; `magneto` is a multi-head
//!   echo feeding the tank; `nonlinear`/`reflections` are feedback-free
//!   multitap **bursts** (a gated/reverse envelope is not a decay loop — it
//!   is a finite window of taps, so the "physics-defying" shapes stay
//!   trivially bounded).
//! - **In-tank inserts**: shimmer's pitch-shifted regeneration (soft-clipped
//!   before re-entry, so a cranked amount drones instead of running away —
//!   the same `tanh` bound as the delay family's self-oscillation), spring's
//!   dispersive chirp cascade (2nd-order allpasses: unity magnitude, so the
//!   loop stays stable), chorale's vowel formants (out-of-loop bandpasses —
//!   resonant boosts never enter the feedback path), swell's input envelope,
//!   bloom's regenerative diffusion loop (loop gain = the knob, capped 0.85).
//!
//! `tone` stays a damping corner in Hz (the v4 `reverb` pedal's key/range —
//! old presets carry over verbatim onto `hall`, the migration target, which
//! at defaults is the M5 topology: scale 1.0, two diffusers, mod 0, neutral
//! low end). `lowend` shapes lows without touching stability: below neutral
//! it strengthens an **in-loop** highpass (loss-only), above neutral it
//! boosts an **input** low shelf (outside the loop).
//!
//! Stereo (M7 rule): one mono-fed core; L/R take orthogonal ±1 Hadamard tap
//! rows of the same lines — decorrelated tails, no doubled feedback cost.

mod bloom;
mod chorale;
mod cloud;
mod hall;
mod magneto;
mod nonlinear;
mod plate;
mod reflections;
mod room;
mod shimmer;
mod spring;
mod swell;

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::blocks::biquad::Biquad;
use crate::blocks::smooth::Smoothed;
use crate::blocks::{onepole_hz, onepole_ms};

const N: usize = 8;
/// Base line lengths in ms (scale 1.0) — spread, mutually incommensurate.
const LINE_MS: [f32; N] = [29.7, 37.1, 41.9, 47.3, 53.9, 61.3, 71.9, 79.7];
/// The largest `size` scale any voice reaches, for buffer capacity.
const MAX_SCALE: f32 = 1.85;
/// Input diffuser delays; a voice uses the first `diff_count` of them.
const DIFF_MS: [f32; 4] = [5.1, 7.9, 13.7, 23.3];
/// Wet tail level: Σ of 8 unit-scale lines needs pulling down.
const WET_SCALE: f32 = 0.35;
/// Output tap signs per channel: two orthogonal Hadamard rows, so L and R
/// hear the same tail energy but decorrelated.
const OUT_L: [f32; N] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const OUT_R: [f32; N] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
/// Read-head wander headroom under `mod`, in ms.
const MOD_HEADROOM_MS: f32 = 8.0;
/// Golden-angle phase offsets decorrelate the per-line LFO copies.
const GOLDEN_ANGLE: f32 = 2.399_963;

// --- voice registry ---

/// The reverb family, in menu order. `hall` leads because it is the M5
/// voicing and the v4→v5 migration target; pinned to
/// `lh_core::preset::REVERB_PEDALS` by a test below. Append-only.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "reverb",
    name: "Reverb",
    pedals: &[
        &hall::DESC,
        &room::DESC,
        &plate::DESC,
        &spring::DESC,
        &swell::DESC,
        &bloom::DESC,
        &cloud::DESC,
        &chorale::DESC,
        &shimmer::DESC,
        &magneto::DESC,
        &nonlinear::DESC,
        &reflections::DESC,
    ],
};

pub const VOICE_COUNT: usize = 12;

/// The voice registry, aligned with [`FAMILY`]`.pedals`.
pub static VOICES: [VoiceDef; VOICE_COUNT] = [
    hall::VOICE,
    room::VOICE,
    plate::VOICE,
    spring::VOICE,
    swell::VOICE,
    bloom::VOICE,
    cloud::VOICE,
    chorale::VOICE,
    shimmer::VOICE,
    magneto::VOICE,
    nonlinear::VOICE,
    reflections::VOICE,
];

/// Which engine control a voice's param position drives. Explicit semantics
/// (unlike delay's generic ModA/ModB) because twelve voices share little.
#[derive(Clone, Copy, PartialEq)]
enum Ctl {
    Decay,
    Predelay,
    Mix,
    Tone,
    Mod,
    /// Tank line scale (tank kinds) or the early-reflection window
    /// (`reflections`), over the voice's `scale_min..scale_max`.
    Size,
    /// Low-frequency character, 0.5 neutral: below = in-loop low cut
    /// (shorter low RT), above = input low-shelf boost (thicker lows).
    LowEnd,
    /// Room: input diffuser strength.
    Diffusion,
    /// Spring: drive into the tank (input soft-clip).
    Dwell,
    /// Spring: number of parallel dispersion chains (stepped 1–3).
    Springs,
    /// Swell: attack ramp time.
    Rise,
    /// Swell: what fades in — reverb only, or dry + reverb (stepped).
    SwellMode,
    /// Bloom: regeneration around the diffusion loop.
    Feedback,
    /// Bloom: regeneration loop length.
    Length,
    /// Cloud: extra diffusion stages blend.
    Haze,
    /// Chorale: vowel morph A→E→I→O→U.
    Vowel,
    /// Chorale: formant blend into the wet.
    Intensity,
    /// Shimmer: pitch-shifted regeneration amount.
    Amount,
    /// Shimmer: shift interval (stepped).
    Interval,
    /// Magneto: head spacing (echo time).
    Spacing,
    /// Magneto: echo feedback.
    Repeats,
    /// Magneto: number of playback heads (stepped 1–4).
    Heads,
    /// Nonlinear/reflections: burst envelope / tap pattern (stepped).
    Shape,
}

/// Structural skeleton of a voice.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// FDN tank (with optional inserts/flags).
    Tank,
    /// Multi-head echo line feeding the tank.
    Magneto,
    /// Feedback-free multitap burst, envelope shaped by the `shape` knob,
    /// window = `decay` (nonlinear).
    Shaped,
    /// Feedback-free early-reflection tap table, window = `size`
    /// (reflections).
    Early,
}

/// In-tank insert, chosen per voice.
#[derive(Clone, Copy, PartialEq)]
enum Insert {
    None,
    /// Pitch-shifted tail re-entry (shimmer).
    Shimmer,
    /// Out-of-loop vowel bandpasses on the wet (chorale).
    Formant,
    /// Dispersive allpass cascade on the tank input (spring).
    Chirp,
}

/// One voice's faceplate, param→control routing (same length as the
/// faceplate), and voicing constants. The engine reads these in the hot
/// loop instead of dispatching through a trait.
pub struct VoiceDef {
    pub desc: &'static EffectDesc,
    controls: &'static [Ctl],
    kind: Kind,
    insert: Insert,
    /// Tank line scale at Size 0 and 1 (geometric sweep); equal = fixed
    /// scale (no Size knob). For `Early`, the reflection window in ms.
    scale_min: f32,
    scale_max: f32,
    /// Input diffusers in use (first `diff_count` of [`DIFF_MS`]) and their
    /// feedback coefficient.
    diff_count: usize,
    diff_g: f32,
    /// Tank read-head modulation: LFO rate, max deviation at `mod` = 1.
    lfo_hz: f32,
    mod_max_ms: f32,
    /// Input envelope (swell) and regenerative diffusion loop (bloom).
    swell: bool,
    bloom: bool,
    /// Per-voice wet trim on top of [`WET_SCALE`] (burst kinds normalize
    /// their tap tables through this).
    wet_gain: f32,
}

// --- shared faceplate parameter constructors ---
// Every voice reuses these keys so shared knobs mean the same thing across
// pedals (and the old flat `reverb` params migrate cleanly onto `hall`).

const fn decay_param(min: f32, max: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "decay",
        name: "Decay",
        unit: "s",
        range: Range::Log { min, max },
        default,
        smoothing_ms: 80.0,
    }
}

const fn predelay_param(max_ms: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "predelay",
        name: "Predelay",
        unit: "ms",
        range: Range::Linear {
            min: 0.0,
            max: max_ms,
        },
        default,
        smoothing_ms: 120.0,
    }
}

const fn mix_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 30.0,
    }
}

/// Damping corner in Hz (dark ⇄ bright), the v4 `reverb` key and unit.
const fn tone_param(min_hz: f32, max_hz: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "tone",
        name: "Tone",
        unit: "Hz",
        range: Range::Log {
            min: min_hz,
            max: max_hz,
        },
        default,
        smoothing_ms: 60.0,
    }
}

const fn mod_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "mod",
        name: "Mod",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 50.0,
    }
}

/// A generic 0..1 character knob (size, lowend, dwell, haze, vowel, …).
const fn knob_param(key: &'static str, name: &'static str, default: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 60.0,
    }
}

const fn stepped_param(
    key: &'static str,
    name: &'static str,
    labels: &'static [&'static str],
    default: f32,
) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Stepped { labels },
        default,
        smoothing_ms: 0.0,
    }
}

// --- primitives ---

/// Fixed-capacity circular buffer with an interpolated read behind the
/// write head. One primitive serves predelay, tank lines, the bloom loop,
/// the magneto echo, and the burst window.
struct ILine {
    buf: Vec<f32>,
    write: usize,
}

impl ILine {
    fn empty() -> Self {
        Self {
            buf: Vec::new(),
            write: 0,
        }
    }

    fn with_ms(ms: f32, sample_rate: f32) -> Self {
        let cap = (ms * 1e-3 * sample_rate) as usize + 4;
        Self {
            buf: vec![0.0; cap],
            write: 0,
        }
    }

    /// Interpolated read `delay_smp` behind the write head, clamped so the
    /// read can never wrap past it.
    #[inline]
    fn read_at(&self, delay_smp: f32) -> f32 {
        let len = self.buf.len();
        let d = delay_smp.clamp(1.0, (len - 2) as f32);
        let rp = self.write as f32 - d + len as f32;
        let i0 = rp as usize;
        let frac = rp - i0 as f32;
        let s0 = self.buf[i0 % len];
        let s1 = self.buf[(i0 + 1) % len];
        s0 + frac * (s1 - s0)
    }

    #[inline]
    fn push(&mut self, value: f32) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.buf.len();
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }
}

/// Schroeder allpass over an [`ILine`]; delay and coefficient are passed per
/// call so one allocation serves every voice's diffusion recipe.
struct ApLine {
    line: ILine,
}

impl ApLine {
    #[inline]
    fn process(&mut self, x: f32, delay_smp: f32, g: f32) -> f32 {
        let delayed = self.line.read_at(delay_smp);
        let input = x + g * delayed;
        self.line.push(input);
        delayed - g * input
    }
}

/// Unity small-signal, bounded loud: `tanh(drive·x)/drive` (RT rule 7 —
/// every regenerative extra stays finite forever).
#[inline]
fn soft_clip(x: f32, drive: f32) -> f32 {
    (x * drive).tanh() / drive
}

/// Delay-line pitch shifter: two read taps swept by a phasor, crossfaded by
/// a sine window (the classic "doppler" shifter — no FFT, RT-safe). Grain
/// ~64 ms: long enough to track low notes, short enough to stay a texture.
struct PitchShift {
    line: ILine,
    phase: f32,
}

const GRAIN_MS: f32 = 64.0;

impl PitchShift {
    #[inline]
    fn process(&mut self, x: f32, ratio: f32, grain_smp: f32) -> f32 {
        self.line.push(x);
        // Read distance shrinks (up-shift) or grows (down-shift) over the
        // grain; two taps half a grain apart hide each wrap under the other
        // tap's window peak.
        self.phase += (1.0 - ratio) / grain_smp;
        self.phase -= self.phase.floor(); // wrap into [0, 1)
        let p2 = {
            let p = self.phase + 0.5;
            p - p.floor()
        };
        let w1 = (std::f32::consts::PI * self.phase).sin();
        let w2 = (std::f32::consts::PI * p2).sin();
        let t1 = self.line.read_at(1.0 + self.phase * grain_smp);
        let t2 = self.line.read_at(1.0 + p2 * grain_smp);
        w1 * t1 + w2 * t2
    }

    fn clear(&mut self) {
        self.line.clear();
        self.phase = 0.0;
    }
}

/// Shimmer intervals: label → pitch ratio(s). "oct+5th" runs both shifters.
pub const INTERVALS: &[&str] = &["+octave", "+5th", "-octave", "oct+5th"];
const INTERVAL_RATIOS: [(f32, f32); 4] = [(2.0, 0.0), (1.5, 0.0), (0.5, 0.0), (2.0, 1.5)];

/// Chorale vowel anchors (F1, F2 in Hz): A → E → I → O → U. The vowel knob
/// morphs geometrically between neighbors.
const VOWELS: [(f32, f32); 5] = [
    (800.0, 1_150.0), // A
    (430.0, 2_100.0), // E
    (300.0, 2_500.0), // I
    (500.0, 850.0),   // O
    (350.0, 700.0),   // U
];
const FORMANT_Q: [f32; 2] = [5.0, 8.0];
/// Two narrow bandpasses lose broadband energy; make the colored branch
/// comparable to the plain tail at full intensity.
const FORMANT_MAKEUP: f32 = 2.2;

/// Spring dispersion: per-spring chirp base frequencies (parallel springs
/// are detuned) and the cascade layout. Fixed at prepare time.
const SPRING_BASE_HZ: [f32; 3] = [2_350.0, 2_650.0, 2_050.0];
const CHIRP_STAGES: usize = 6;
const CHIRP_Q: f32 = 2.8;

/// Magneto head level taper (head 1 is loudest).
const HEAD_GAIN: [f32; 4] = [1.0, 0.85, 0.72, 0.61];
const MAGNETO_DRIVE: f32 = 1.3;
pub const HEAD_LABELS: &[&str] = &["1", "2", "3", "4"];

/// Nonlinear burst: 24 taps spread over the window with golden-ratio jitter
/// and alternating polarity; the `shape` knob picks the envelope law.
const BURST_TAPS: usize = 24;
pub const NONLINEAR_SHAPES: &[&str] = &["gate", "reverse", "swoosh"];

/// Early-reflection tables: 12 taps × (position 0..1, gain, L sign, R sign)
/// per room shape. Irregular positions, no feedback — pure pattern.
pub const REFLECTION_SHAPES: &[&str] = &["studio", "chamber", "dome"];
const ER_TAPS: usize = 12;
type ErTap = (f32, f32, f32, f32);
const ER_TABLES: [[ErTap; ER_TAPS]; 3] = [
    // studio: tight, fast falloff
    [
        (0.061, 0.92, 1.0, 0.6),
        (0.113, 0.78, -0.7, 1.0),
        (0.171, 0.71, 1.0, -0.8),
        (0.229, 0.62, -0.9, -0.5),
        (0.293, 0.55, 0.6, 1.0),
        (0.367, 0.48, -1.0, 0.7),
        (0.449, 0.41, 0.8, -1.0),
        (0.541, 0.35, -0.5, 0.9),
        (0.647, 0.29, 1.0, 0.5),
        (0.761, 0.24, -0.8, -1.0),
        (0.883, 0.19, 0.7, 0.8),
        (1.000, 0.15, -1.0, -0.6),
    ],
    // chamber: sparser start, longer body
    [
        (0.089, 0.88, 1.0, -0.6),
        (0.163, 0.80, -0.8, 1.0),
        (0.251, 0.74, 0.7, 0.9),
        (0.331, 0.66, -1.0, -0.7),
        (0.421, 0.60, 0.9, -1.0),
        (0.503, 0.55, -0.6, 0.8),
        (0.587, 0.50, 1.0, 0.6),
        (0.673, 0.45, -0.9, -0.9),
        (0.757, 0.41, 0.5, 1.0),
        (0.839, 0.37, -1.0, 0.7),
        (0.919, 0.34, 0.8, -0.8),
        (1.000, 0.31, -0.7, 1.0),
    ],
    // dome: clustered slap + late focus
    [
        (0.047, 0.95, 1.0, 1.0),
        (0.079, 0.83, -0.9, 0.8),
        (0.107, 0.74, 0.8, -0.9),
        (0.139, 0.66, -0.7, -0.7),
        (0.401, 0.58, 1.0, -0.6),
        (0.439, 0.52, -1.0, 0.9),
        (0.483, 0.47, 0.6, 1.0),
        (0.523, 0.42, -0.8, -1.0),
        (0.827, 0.36, 0.9, 0.7),
        (0.863, 0.32, -0.6, -0.8),
        (0.907, 0.28, 1.0, 0.5),
        (1.000, 0.25, -0.9, 1.0),
    ],
];

/// Swell onset detector time constants.
const SWELL_FAST_MS: f32 = 2.0;
const SWELL_SLOW_MS: f32 = 150.0;

pub struct Reverb {
    sample_rate: f32,
    voice: usize,

    // knobs (smoothed); unused ones idle at their defaults
    decay_s: Smoothed,
    predelay_ms: Smoothed,
    mix: Smoothed,
    tone_hz: Smoothed,
    depth: Smoothed,
    size: Smoothed,
    lowend: Smoothed,
    diffusion: Smoothed,
    dwell: Smoothed,
    rise_ms: Smoothed,
    bloom_fb: Smoothed,
    bloom_len: Smoothed,
    haze: Smoothed,
    vowel: Smoothed,
    intensity: Smoothed,
    amount: Smoothed,
    spacing_ms: Smoothed,
    repeats: Smoothed,
    // stepped knobs (snap)
    interval: usize,
    heads: usize,
    springs: usize,
    swell_mode: usize,
    shape: usize,

    // tank
    predelay: ILine,
    diff: Vec<ApLine>,
    lines: Vec<ILine>,
    line_len: [f32; N],
    line_len_inc: [f32; N],
    line_gain: [f32; N],
    damp: [f32; N],
    damp_coeff: f32,
    lowcut: [f32; N],
    lowcut_coeff: f32,
    shelf: Biquad,
    shelf_db: f32,
    lfo_phase: f32,
    lfo_cos: [f32; N],
    lfo_sin: [f32; N],

    // derived-coefficient cache (skip rebuilds while knobs are settled)
    cache_dirty: bool,

    // swell
    env_fast: f32,
    env_slow: f32,
    swell_gain: f32,

    // bloom
    bloom_line: ILine,

    // shimmer
    shifters: [PitchShift; 2],
    shim_hold: f32,

    // chorale (per channel × formant)
    formants: [[Biquad; 2]; 2],

    // spring
    chirps: Vec<[Biquad; CHIRP_STAGES]>,

    // magneto
    echo: ILine,
    echo_lp: f32,

    // burst (nonlinear + reflections share the window line)
    burst: ILine,
    burst_lp: [f32; 2],
    burst_pos: [f32; BURST_TAPS],
    burst_sign: [(f32, f32); BURST_TAPS],
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    pub fn new() -> Self {
        // Defaults mirror the hall faceplate (voice 0); a pedal switch
        // re-sends the incoming voice's values from the control shadow.
        let mut burst_pos = [0.0f32; BURST_TAPS];
        let mut burst_sign = [(1.0f32, 1.0f32); BURST_TAPS];
        let mut acc = 0.0f32;
        for (i, (pos, sign)) in burst_pos.iter_mut().zip(&mut burst_sign).enumerate() {
            // Golden-ratio jitter around an even grid: dense, aperiodic.
            acc += 0.618_034;
            acc -= acc.floor();
            *pos = ((i as f32 + 0.35 + 0.5 * acc) / BURST_TAPS as f32).min(1.0);
            *sign = (
                if i % 2 == 0 { 1.0 } else { -1.0 },
                if (i / 2) % 2 == 0 { 1.0 } else { -1.0 },
            );
        }
        let mut lfo_cos = [0.0f32; N];
        let mut lfo_sin = [0.0f32; N];
        for i in 0..N {
            let phi = GOLDEN_ANGLE * i as f32;
            lfo_cos[i] = phi.cos();
            lfo_sin[i] = phi.sin();
        }
        Self {
            sample_rate: 48_000.0,
            voice: 0,
            decay_s: Smoothed::new(1.8),
            predelay_ms: Smoothed::new(20.0),
            mix: Smoothed::new(0.3),
            tone_hz: Smoothed::new(5_000.0),
            depth: Smoothed::new(0.0),
            size: Smoothed::new(0.5),
            lowend: Smoothed::new(0.5),
            diffusion: Smoothed::new(0.6),
            dwell: Smoothed::new(0.35),
            rise_ms: Smoothed::new(600.0),
            bloom_fb: Smoothed::new(0.35),
            bloom_len: Smoothed::new(0.5),
            haze: Smoothed::new(0.6),
            vowel: Smoothed::new(0.35),
            intensity: Smoothed::new(0.6),
            amount: Smoothed::new(0.5),
            spacing_ms: Smoothed::new(140.0),
            repeats: Smoothed::new(0.4),
            interval: 0,
            heads: 2,
            springs: 1,
            swell_mode: 0,
            shape: 0,
            predelay: ILine::empty(),
            diff: Vec::new(),
            lines: Vec::new(),
            line_len: [1.0; N],
            line_len_inc: [0.0; N],
            line_gain: [0.0; N],
            damp: [0.0; N],
            damp_coeff: 1.0,
            lowcut: [0.0; N],
            lowcut_coeff: 0.0,
            shelf: Biquad::default(),
            shelf_db: 0.0,
            lfo_phase: 0.0,
            lfo_cos,
            lfo_sin,
            cache_dirty: true,
            env_fast: 0.0,
            env_slow: 0.0,
            swell_gain: 0.0,
            bloom_line: ILine::empty(),
            shifters: [
                PitchShift {
                    line: ILine::empty(),
                    phase: 0.0,
                },
                PitchShift {
                    line: ILine::empty(),
                    phase: 0.25,
                },
            ],
            shim_hold: 0.0,
            formants: [[Biquad::default(); 2]; 2],
            chirps: Vec::new(),
            echo: ILine::empty(),
            echo_lp: 0.0,
            burst: ILine::empty(),
            burst_lp: [0.0; 2],
            burst_pos,
            burst_sign,
        }
    }

    /// The tank line scale for the active voice at the current size knob.
    #[inline]
    fn scale(&self, def: &VoiceDef) -> f32 {
        let (lo, hi) = (def.scale_min, def.scale_max);
        if lo == hi {
            lo
        } else {
            lo * (hi / lo).powf(self.size.current().clamp(0.0, 1.0))
        }
    }

    /// Rebuild block-rate coefficients (line lengths/gains, damping, low
    /// end, formants) — only called while a relevant knob is moving.
    fn refresh(&mut self, def: &VoiceDef, block_len: usize) {
        let sr = self.sample_rate;
        let ms_to_smp = sr * 1e-3;
        let t60 = self.decay_s.current().max(0.05);
        let scale = self.scale(def);
        let inv_block = 1.0 / block_len.max(1) as f32;
        for ((len, inc), (gain, &ms)) in self
            .line_len
            .iter()
            .zip(&mut self.line_len_inc)
            .zip(self.line_gain.iter_mut().zip(&LINE_MS))
        {
            let target = ms * scale * ms_to_smp;
            *inc = (target - len) * inv_block;
            *gain = 10f32.powf(-3.0 * (ms * scale * 1e-3) / t60);
        }
        self.damp_coeff = onepole_hz(self.tone_hz.current(), sr);
        let le = self.lowend.current();
        if le < 0.5 {
            let hz = 20.0 + (0.5 - le) * 2.0 * 200.0;
            self.lowcut_coeff = onepole_hz(hz, sr);
        } else {
            self.lowcut_coeff = 0.0;
        }
        let shelf_db = ((le - 0.5).max(0.0) * 2.0 * 6.0 * 100.0).round() / 100.0;
        if shelf_db != self.shelf_db {
            self.shelf_db = shelf_db;
            self.shelf.set_low_shelf(sr, 240.0, shelf_db);
        }
        if def.insert == Insert::Formant {
            let v = self.vowel.current().clamp(0.0, 1.0) * (VOWELS.len() - 1) as f32;
            let seg = (v as usize).min(VOWELS.len() - 2);
            let t = v - seg as f32;
            let (a1, a2) = VOWELS[seg];
            let (b1, b2) = VOWELS[seg + 1];
            let f1 = a1 * (b1 / a1).powf(t);
            let f2 = a2 * (b2 / a2).powf(t);
            for ch in &mut self.formants {
                ch[0].set_bandpass(sr, f1, FORMANT_Q[0]);
                ch[1].set_bandpass(sr, f2, FORMANT_Q[1]);
            }
        }
        self.cache_dirty = false;
    }

    /// True while any knob that feeds [`Self::refresh`] is still gliding.
    fn coeffs_moving(&self, def: &VoiceDef) -> bool {
        !self.decay_s.is_settled()
            || !self.tone_hz.is_settled()
            || !self.size.is_settled()
            || !self.lowend.is_settled()
            || (def.insert == Insert::Formant && !self.vowel.is_settled())
    }

    /// One sample through the FDN tank. `input` is the (diffused, possibly
    /// insert-augmented) mono feed; returns the raw (unscaled) L/R wet taps.
    #[inline]
    fn tank_step(&mut self, input: f32) -> (f32, f32) {
        let depth = self.depth.current();
        let (s, c) = if depth > 0.0 {
            self.lfo_phase.sin_cos()
        } else {
            (0.0, 0.0)
        };
        let def = &VOICES[self.voice];
        let dev = depth * def.mod_max_ms * self.sample_rate * 1e-3;
        let mut v = [0.0f32; N];
        let mut sum = 0.0;
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for i in 0..N {
            self.line_len[i] += self.line_len_inc[i];
            let off = if depth > 0.0 {
                dev * (s * self.lfo_cos[i] + c * self.lfo_sin[i])
            } else {
                0.0
            };
            let tail = self.lines[i].read_at(self.line_len[i] + off);
            wet_l += tail * OUT_L[i];
            wet_r += tail * OUT_R[i];
            self.damp[i] += self.damp_coeff * (tail - self.damp[i]);
            if self.damp[i].abs() < 1e-20 {
                self.damp[i] = 0.0;
            }
            let mut fed = self.damp[i];
            if self.lowcut_coeff > 0.0 {
                self.lowcut[i] += self.lowcut_coeff * (fed - self.lowcut[i]);
                if self.lowcut[i].abs() < 1e-20 {
                    self.lowcut[i] = 0.0;
                }
                fed -= self.lowcut[i];
            }
            v[i] = self.line_gain[i] * fed;
            sum += v[i];
        }
        let house = 2.0 / N as f32 * sum;
        for (line, fed) in self.lines.iter_mut().zip(&v) {
            line.push(input + fed - house);
        }
        (wet_l, wet_r)
    }
}

impl Effect for Reverb {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.voice
    }

    /// Reverbs have the longest tails of anything in the chain — a long
    /// hall/bloom decay rings for many seconds. The spill lane ends it by
    /// detecting silence (PRD 010); this is the conservative hint.
    fn tail_seconds(&self) -> f32 {
        12.0
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal != self.voice && pedal < VOICE_COUNT {
            self.voice = pedal;
            // Keep the tank/echo/burst buffers so tails ring through the
            // switch; drop insert filter memory and force a coefficient
            // rebuild so the incoming voicing takes hold cleanly (the
            // control side re-sends the incoming pedal's values, PRD 001).
            for ch in &mut self.formants {
                for f in ch {
                    f.reset();
                }
            }
            for spring in &mut self.chirps {
                for ap in spring {
                    ap.reset();
                }
            }
            for s in &mut self.shifters {
                s.clear();
            }
            self.shifters[1].phase = 0.25; // keep the dual taps decorrelated
            self.shim_hold = 0.0;
            self.env_fast = 0.0;
            self.env_slow = 0.0;
            self.swell_gain = 0.0;
            self.lowcut = [0.0; N];
            self.cache_dirty = true;
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        let sr = self.sample_rate;
        // Everything is allocated here, for every voice — a pedal switch on
        // the audio thread touches no memory (RT rule 1).
        self.predelay = ILine::with_ms(260.0, sr);
        self.diff = DIFF_MS
            .iter()
            .map(|&ms| ApLine {
                line: ILine::with_ms(ms + 1.0, sr),
            })
            .collect();
        self.lines = LINE_MS
            .iter()
            .map(|&ms| ILine::with_ms(ms * MAX_SCALE + MOD_HEADROOM_MS, sr))
            .collect();
        self.bloom_line = ILine::with_ms(220.0, sr);
        for s in &mut self.shifters {
            s.line = ILine::with_ms(GRAIN_MS + 4.0, sr);
        }
        self.chirps = (0..3)
            .map(|s| {
                let mut cascade = [Biquad::default(); CHIRP_STAGES];
                for (k, ap) in cascade.iter_mut().enumerate() {
                    let fc = SPRING_BASE_HZ[s] * (0.78 + 0.075 * k as f32);
                    ap.set_allpass(sr, fc, CHIRP_Q);
                }
                cascade
            })
            .collect();
        self.echo = ILine::with_ms(4.0 * 400.0 + 10.0, sr);
        self.burst = ILine::with_ms(1_550.0, sr);

        for (smoothed, ms) in [
            (&mut self.decay_s, 80.0),
            (&mut self.predelay_ms, 120.0),
            (&mut self.mix, 30.0),
            (&mut self.tone_hz, 60.0),
            (&mut self.depth, 50.0),
            (&mut self.size, 120.0),
            (&mut self.lowend, 60.0),
            (&mut self.diffusion, 60.0),
            (&mut self.dwell, 30.0),
            (&mut self.rise_ms, 60.0),
            (&mut self.bloom_fb, 30.0),
            (&mut self.bloom_len, 120.0),
            (&mut self.haze, 60.0),
            (&mut self.vowel, 60.0),
            (&mut self.intensity, 30.0),
            (&mut self.amount, 30.0),
            (&mut self.spacing_ms, 150.0),
            (&mut self.repeats, 20.0),
        ] {
            smoothed.configure(ms, sample_rate);
            smoothed.snap_to_target();
        }
        // Start the lines at the active voice's settled lengths.
        let scale = self.scale(&VOICES[self.voice]);
        for (len, &ms) in self.line_len.iter_mut().zip(&LINE_MS) {
            *len = ms * scale * sr * 1e-3;
        }
        self.line_len_inc = [0.0; N];
        self.shelf_db = f32::NAN; // force a shelf rebuild
        self.cache_dirty = true;
        self.reset();
    }

    fn reset(&mut self) {
        self.predelay.clear();
        for ap in &mut self.diff {
            ap.line.clear();
        }
        for line in &mut self.lines {
            line.clear();
        }
        self.damp = [0.0; N];
        self.lowcut = [0.0; N];
        self.shelf.reset();
        self.lfo_phase = 0.0;
        self.env_fast = 0.0;
        self.env_slow = 0.0;
        self.swell_gain = 0.0;
        self.bloom_line.clear();
        for s in &mut self.shifters {
            s.clear();
        }
        self.shifters[1].phase = 0.25;
        self.shim_hold = 0.0;
        for ch in &mut self.formants {
            for f in ch {
                f.reset();
            }
        }
        for spring in &mut self.chirps {
            for ap in spring {
                ap.reset();
            }
        }
        self.echo.clear();
        self.echo_lp = 0.0;
        self.burst.clear();
        self.burst_lp = [0.0; 2];
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let def = &VOICES[self.voice];
        let (Some(ctl), Some(param)) = (def.controls.get(index), def.desc.params.get(index)) else {
            return; // out-of-range indices are ignored (Effect contract)
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Decay => self.decay_s.set_target(real),
            Ctl::Predelay => self.predelay_ms.set_target(real),
            Ctl::Mix => self.mix.set_target(real),
            Ctl::Tone => self.tone_hz.set_target(real),
            Ctl::Mod => self.depth.set_target(real),
            Ctl::Size => self.size.set_target(real),
            Ctl::LowEnd => self.lowend.set_target(real),
            Ctl::Diffusion => self.diffusion.set_target(real),
            Ctl::Dwell => self.dwell.set_target(real),
            Ctl::Springs => self.springs = real as usize,
            Ctl::Rise => self.rise_ms.set_target(real),
            Ctl::SwellMode => self.swell_mode = real as usize,
            Ctl::Feedback => self.bloom_fb.set_target(real),
            Ctl::Length => self.bloom_len.set_target(real),
            Ctl::Haze => self.haze.set_target(real),
            Ctl::Vowel => self.vowel.set_target(real),
            Ctl::Intensity => self.intensity.set_target(real),
            Ctl::Amount => self.amount.set_target(real),
            Ctl::Interval => self.interval = real as usize,
            Ctl::Spacing => self.spacing_ms.set_target(real),
            Ctl::Repeats => self.repeats.set_target(real),
            Ctl::Heads => self.heads = real as usize,
            Ctl::Shape => self.shape = real as usize,
        }
        // Decay/tone/size/lowend/vowel feed cached coefficients.
        self.cache_dirty = true;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.lines.is_empty() {
            return; // prepare() not called yet
        }
        let def = &VOICES[self.voice];
        let sr = self.sample_rate;
        let ms_to_smp = sr * 1e-3;
        if self.cache_dirty || self.coeffs_moving(def) {
            // Advance the knob smoothers by one block's worth *through*
            // refresh: refresh() reads .current(), the per-sample loop below
            // ticks them, so coefficients lag the knobs by at most a block.
            self.refresh(def, left.len());
        } else {
            for inc in &mut self.line_len_inc {
                *inc = 0.0;
            }
        }
        let lfo_inc = std::f32::consts::TAU * def.lfo_hz / sr;
        let fast_c = onepole_ms(SWELL_FAST_MS, sr as u32);
        let slow_c = onepole_ms(SWELL_SLOW_MS, sr as u32);
        let grain_smp = GRAIN_MS * ms_to_smp;

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (dry_l, dry_r) = (*l, *r);
            let dry = 0.5 * (dry_l + dry_r); // the core is mono-fed
            let mix = self.mix.tick();
            self.tone_hz.tick();
            self.decay_s.tick();
            self.size.tick();
            self.lowend.tick();
            self.depth.tick();

            // Swell (mode "dry+reverb") gates the dry with the same ramp.
            let mut dry_gain = 1.0;
            let (mut wet_l, mut wet_r);
            match def.kind {
                Kind::Tank => {
                    let pre = self
                        .predelay
                        .read_at(self.predelay_ms.tick() * ms_to_smp + 1.0);
                    self.predelay.push(dry);

                    // Swell: ramp the wet feed (and optionally the dry) in
                    // from silence on every detected onset.
                    let mut feed = pre;
                    if def.swell {
                        let mag = dry.abs();
                        self.env_fast += fast_c * (mag - self.env_fast);
                        self.env_slow += slow_c * (mag - self.env_slow);
                        if self.env_fast > 3.0 * self.env_slow + 1e-3 {
                            self.swell_gain = 0.0; // new note: restart the ramp
                        }
                        let rise_smp = (self.rise_ms.tick() * ms_to_smp).max(1.0);
                        self.swell_gain = (self.swell_gain + 1.0 / rise_smp).min(1.0);
                        feed *= self.swell_gain;
                        if self.swell_mode == 1 {
                            dry_gain = self.swell_gain;
                        }
                    }

                    if self.shelf_db > 0.0 {
                        feed = self.shelf.process_sample(feed);
                    }

                    // Spring: dwell drive, then the dispersive chirp bank.
                    if def.insert == Insert::Chirp {
                        let drive = 0.8 + 5.0 * self.dwell.tick();
                        feed = soft_clip(feed, drive);
                        let active = (self.springs + 1).min(self.chirps.len());
                        let mut sum = 0.0;
                        for cascade in self.chirps.iter_mut().take(active) {
                            let mut y = feed;
                            for ap in cascade {
                                y = ap.process_sample(y);
                            }
                            sum += y;
                        }
                        feed = sum / active as f32;
                    }

                    // Input diffusion; room sweeps g, cloud fades stages
                    // 3/4 in with haze.
                    let g = if def.controls.contains(&Ctl::Diffusion) {
                        0.3 + 0.5 * self.diffusion.tick()
                    } else {
                        def.diff_g
                    };
                    let haze = if def.controls.contains(&Ctl::Haze) {
                        self.haze.tick()
                    } else {
                        0.0
                    };
                    let mut x = feed;
                    for (i, ap) in self.diff.iter_mut().take(def.diff_count).enumerate() {
                        let stage_g = if i >= 2 && haze > 0.0 { 0.68 * haze } else { g };
                        x = ap.process(x, DIFF_MS[i] * ms_to_smp, stage_g);
                    }

                    // Bloom: regenerative loop around the diffused feed.
                    if def.bloom {
                        let d = (40.0 + 160.0 * self.bloom_len.tick()) * ms_to_smp;
                        let back = self.bloom_line.read_at(d);
                        x += self.bloom_fb.tick() * back;
                        self.bloom_line.push(x);
                    }

                    // Shimmer: pitch-shifted tail re-entry, soft-clipped so
                    // the regeneration loop is bounded at any knob setting.
                    if def.insert == Insert::Shimmer {
                        let amt = 0.72 * self.amount.tick();
                        let (r1, r2) = INTERVAL_RATIOS[self.interval.min(3)];
                        let fed = soft_clip(self.shim_hold, 1.3);
                        let mut shifted = self.shifters[0].process(fed, r1, grain_smp);
                        if r2 > 0.0 {
                            shifted =
                                0.6 * (shifted + self.shifters[1].process(fed, r2, grain_smp));
                        }
                        x += amt * shifted;
                    }

                    let (tl, tr) = self.tank_step(x);
                    wet_l = tl;
                    wet_r = tr;
                    if def.insert == Insert::Shimmer {
                        self.shim_hold = 0.5 * (wet_l + wet_r) * WET_SCALE;
                    }

                    // Chorale: vowel bandpasses blend into the wet,
                    // out-of-loop (resonance never re-enters the feedback).
                    if def.insert == Insert::Formant {
                        self.vowel.tick(); // feeds refresh() at block rate
                        let i = self.intensity.tick();
                        let fl = self.formants[0][0].process_sample(wet_l)
                            + self.formants[0][1].process_sample(wet_l);
                        let fr = self.formants[1][0].process_sample(wet_r)
                            + self.formants[1][1].process_sample(wet_r);
                        wet_l += i * (FORMANT_MAKEUP * fl - wet_l);
                        wet_r += i * (FORMANT_MAKEUP * fr - wet_r);
                    }

                    wet_l *= WET_SCALE * def.wet_gain;
                    wet_r *= WET_SCALE * def.wet_gain;
                }
                Kind::Magneto => {
                    // Multi-head echo: heads at k×spacing; the feedback tap
                    // (last head) is soft-clipped and darkened in-loop.
                    let spacing = self.spacing_ms.tick() * ms_to_smp;
                    let wow =
                        self.depth.current() * def.mod_max_ms * ms_to_smp * self.lfo_phase.sin();
                    let heads = (self.heads + 1).min(HEAD_GAIN.len());
                    let mut head_sum = 0.0;
                    for (k, gain) in HEAD_GAIN.iter().take(heads).enumerate() {
                        head_sum += gain * self.echo.read_at((k + 1) as f32 * spacing + wow);
                    }
                    let last = self.echo.read_at(heads as f32 * spacing + wow);
                    let fb = self.repeats.tick();
                    let write = dry + fb * soft_clip(last, MAGNETO_DRIVE);
                    // In-loop tone (block-rate coeff): repeats darken pass
                    // over pass, tape-like.
                    self.echo_lp += self.damp_coeff * (write - self.echo_lp);
                    if self.echo_lp.abs() < 1e-20 {
                        self.echo_lp = 0.0;
                    }
                    self.echo.push(self.echo_lp);

                    // The echo feeds the tank for the wash behind the heads.
                    let mut x = 0.6 * head_sum;
                    for (i, ap) in self.diff.iter_mut().take(def.diff_count).enumerate() {
                        x = ap.process(x, DIFF_MS[i] * ms_to_smp, def.diff_g);
                    }
                    let (tl, tr) = self.tank_step(x);
                    wet_l = 0.85 * head_sum + tl * WET_SCALE;
                    wet_r = 0.85 * head_sum + tr * WET_SCALE;
                    wet_l *= def.wet_gain;
                    wet_r *= def.wet_gain;
                }
                Kind::Shaped | Kind::Early => {
                    // Feedback-free burst: diffuse (nonlinear only), write,
                    // then read the tap fan through the envelope/table.
                    let pre = self
                        .predelay
                        .read_at(self.predelay_ms.tick() * ms_to_smp + 1.0);
                    self.predelay.push(dry);
                    let mut x = pre;
                    for (i, ap) in self.diff.iter_mut().take(def.diff_count).enumerate() {
                        x = ap.process(x, DIFF_MS[i] * ms_to_smp, def.diff_g);
                    }
                    self.burst.push(x);
                    let mut bl = 0.0;
                    let mut br = 0.0;
                    if def.kind == Kind::Shaped {
                        let w = self.decay_s.current() * 1e3 * ms_to_smp;
                        let shape = self.shape.min(NONLINEAR_SHAPES.len() - 1);
                        for t in 0..BURST_TAPS {
                            let p = self.burst_pos[t];
                            let gain = match shape {
                                0 => {
                                    // gate: flat, then a fast final ramp
                                    if p <= 0.8 { 1.0 } else { (1.0 - p) / 0.2 }
                                }
                                1 => p * p, // reverse: rising
                                _ => {
                                    let s = (std::f32::consts::PI * p).sin();
                                    s * s // swoosh: arch
                                }
                            };
                            if gain <= 0.0 {
                                continue;
                            }
                            let tap = self.burst.read_at(p * w + 1.0);
                            let (sl, sr_) = self.burst_sign[t];
                            bl += gain * sl * tap;
                            br += gain * sr_ * tap;
                        }
                    } else {
                        let window = (def.scale_min
                            + (def.scale_max - def.scale_min) * self.size.current())
                            * ms_to_smp;
                        let table = &ER_TABLES[self.shape.min(REFLECTION_SHAPES.len() - 1)];
                        for (p, gain, sl, sr_) in table {
                            let tap = self.burst.read_at(p * window + 1.0);
                            bl += gain * sl * tap;
                            br += gain * sr_ * tap;
                        }
                    }
                    // Out-of-loop tone lowpass (block-rate coeff) on the
                    // burst wet.
                    self.burst_lp[0] += self.damp_coeff * (bl - self.burst_lp[0]);
                    self.burst_lp[1] += self.damp_coeff * (br - self.burst_lp[1]);
                    for lp in &mut self.burst_lp {
                        if lp.abs() < 1e-20 {
                            *lp = 0.0;
                        }
                    }
                    wet_l = self.burst_lp[0] * def.wet_gain;
                    wet_r = self.burst_lp[1] * def.wet_gain;
                }
            }

            self.lfo_phase += lfo_inc;
            if self.lfo_phase >= std::f32::consts::TAU {
                self.lfo_phase -= std::f32::consts::TAU;
            }
            let gl = dry_l * dry_gain;
            let gr = dry_r * dry_gain;
            *l = gl + mix * (wet_l - gl);
            *r = gr + mix * (wet_r - gr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, process_in_blocks, rms, silence, sine};

    const SR: u32 = 48_000;

    /// Voice indices by key, resolved through the registry (tests stay
    /// readable and reordering-proof).
    fn voice_of(key: &str) -> usize {
        FAMILY.pedal_index(key).unwrap()
    }

    /// A prepared reverb on `voice` with that voice's own defaults applied —
    /// exactly what the control side does after `SelectPedal` (PRD 001).
    fn prepared(voice: usize) -> Reverb {
        let mut r = Reverb::new();
        r.prepare(SR);
        r.select_pedal(voice);
        for (i, p) in VOICES[voice].desc.params.iter().enumerate() {
            r.set_param(i, p.default_norm());
        }
        r
    }

    /// Set a param by key with a real-world value on the active voice.
    fn set_by(r: &mut Reverb, key: &str, real: f32) {
        let def = &VOICES[r.voice];
        let i = def.desc.param_index(key).unwrap();
        r.set_param(i, def.desc.params[i].range.to_norm(real));
    }

    /// Warm the smoothers to their targets (renders and discards `secs`).
    fn settle(r: &mut Reverb, secs: f32) {
        let n = (SR as f32 * secs) as usize;
        let mut a = silence(n);
        let mut b = silence(n);
        r.process(&mut a, &mut b);
    }

    /// Render `secs` of stereo impulse response (left returned first).
    fn impulse_response_stereo(r: &mut Reverb, secs: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (SR as f32 * secs) as usize;
        let mut l = vec![0.0f32; n];
        l[0] = 1.0;
        let mut right = l.clone();
        for (a, b) in l.chunks_mut(64).zip(right.chunks_mut(64)) {
            r.process(a, b);
        }
        (l, right)
    }

    fn impulse_response(r: &mut Reverb, secs: f32) -> Vec<f32> {
        impulse_response_stereo(r, secs).0
    }

    fn window_rms(x: &[f32], from_s: f32, to_s: f32) -> f32 {
        let a = (SR as f32 * from_s) as usize;
        let b = ((SR as f32 * to_s) as usize).min(x.len());
        rms(&x[a..b])
    }

    /// Projection of `x` onto `freq` (normalized magnitude) — reads one
    /// frequency's level out of a real rendered signal.
    fn level_at(x: &[f32], freq: f32) -> f64 {
        let w = std::f64::consts::TAU * f64::from(freq) / f64::from(SR);
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (n, v) in x.iter().enumerate() {
            let (ps, pc) = (w * n as f64).sin_cos();
            s += f64::from(*v) * ps;
            c += f64::from(*v) * pc;
        }
        (s * s + c * c).sqrt() / x.len() as f64
    }

    // --- registry ---

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.pedals.len(), VOICES.len());
        assert_eq!(VOICES.len(), VOICE_COUNT);
        for (def, desc) in VOICES.iter().zip(FAMILY.pedals) {
            assert!(std::ptr::eq(def.desc, *desc), "VOICES aligned with FAMILY");
            assert_eq!(def.controls.len(), def.desc.params.len(), "{}", desc.key);
        }
        for (i, a) in FAMILY.pedals.iter().enumerate() {
            for b in &FAMILY.pedals[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
        // The v4→v5 migration references voices by key; pin the registry.
        let keys: Vec<&str> = FAMILY.pedals.iter().map(|p| p.key).collect();
        assert_eq!(keys, lh_core::preset::REVERB_PEDALS);
        // Every voice wears its own faceplate.
        let captions = |key: &str| -> Vec<&str> {
            FAMILY.pedals[voice_of(key)]
                .params
                .iter()
                .map(|p| p.name)
                .collect()
        };
        let shared = ["Decay", "Predelay", "Mix", "Tone", "Mod"];
        assert_eq!(
            captions("hall"),
            [&shared[..], &["Size", "Low End"]].concat()
        );
        assert_eq!(
            captions("room"),
            [&shared[..], &["Size", "Diffusion"]].concat()
        );
        assert_eq!(
            captions("plate"),
            [&shared[..], &["Size", "Low End"]].concat()
        );
        assert_eq!(
            captions("spring"),
            ["Decay", "Predelay", "Mix", "Tone", "Dwell", "Springs"]
        );
        assert_eq!(captions("swell"), [&shared[..], &["Rise", "Mode"]].concat());
        assert_eq!(
            captions("bloom"),
            [&shared[..], &["Feedback", "Length"]].concat()
        );
        assert_eq!(captions("cloud"), [&shared[..], &["Haze"]].concat());
        assert_eq!(
            captions("chorale"),
            [&shared[..], &["Vowel", "Intensity"]].concat()
        );
        assert_eq!(
            captions("shimmer"),
            [&shared[..], &["Amount", "Interval"]].concat()
        );
        assert_eq!(
            captions("magneto"),
            ["Decay", "Spacing", "Mix", "Tone", "Mod", "Repeats", "Heads"]
        );
        assert_eq!(
            captions("nonlinear"),
            ["Decay", "Predelay", "Mix", "Tone", "Shape"]
        );
        assert_eq!(
            captions("reflections"),
            ["Predelay", "Mix", "Tone", "Size", "Shape"]
        );
        // Hall is the migration target: its shared keys keep the v4
        // `reverb` pedal's ranges and defaults exactly.
        let hall = FAMILY.pedals[0];
        assert_eq!(hall.key, "hall");
        let p = |key: &str| &hall.params[hall.param_index(key).unwrap()];
        assert_eq!(p("decay").range, Range::Log { min: 0.2, max: 8.0 });
        assert_eq!(p("decay").default, 1.8);
        assert_eq!(
            p("tone").range,
            Range::Log {
                min: 1_000.0,
                max: 12_000.0
            }
        );
        assert_eq!(p("tone").default, 5_000.0);
        assert_eq!(p("predelay").default, 20.0);
        assert_eq!(p("mix").default, 0.3);
        // …and its new knobs default to neutral (no mod, scale 1.0).
        assert_eq!(p("mod").default, 0.0);
        let scale = VOICES[0].scale_min
            * (VOICES[0].scale_max / VOICES[0].scale_min).powf(p("size").default);
        assert!((scale - 1.0).abs() < 1e-4, "hall noon must be scale 1.0");
    }

    // --- hall keeps the M5 behavior ---

    fn hall_full_wet(t60: f32) -> Reverb {
        let mut r = prepared(voice_of("hall"));
        set_by(&mut r, "decay", t60);
        set_by(&mut r, "mix", 1.0);
        settle(&mut r, 0.5);
        r.reset();
        r
    }

    #[test]
    fn hall_tail_decays_monotonically() {
        let mut r = hall_full_wet(1.0);
        let ir = impulse_response(&mut r, 3.0);
        assert_finite("hall ir", &ir);
        let early = window_rms(&ir, 0.05, 0.30);
        let mid = window_rms(&ir, 0.8, 1.2);
        let late = window_rms(&ir, 2.0, 2.8);
        assert!(
            early > mid && mid > late,
            "tail must decay: {early} {mid} {late}"
        );
        assert!(late < early * 0.01, "late tail too loud: {late} vs {early}");
    }

    #[test]
    fn hall_decay_parameter_stretches_the_tail() {
        let mut short = hall_full_wet(0.3);
        let mut long = hall_full_wet(6.0);
        let ir_short = impulse_response(&mut short, 2.0);
        let ir_long = impulse_response(&mut long, 2.0);
        let at_1s = |ir: &[f32]| window_rms(ir, 0.9, 1.3);
        assert!(
            at_1s(&ir_long) > at_1s(&ir_short) * 10.0,
            "6 s decay must ring much longer than 0.3 s"
        );
    }

    #[test]
    fn hall_predelay_shifts_the_onset() {
        let mut r = hall_full_wet(1.0);
        set_by(&mut r, "predelay", 100.0);
        settle(&mut r, 1.0);
        r.reset();
        let ir = impulse_response(&mut r, 0.5);
        let before = rms(&ir[SR as usize * 5 / 1000..SR as usize * 120 / 1000]);
        let after = window_rms(&ir, 0.14, 0.4);
        assert!(before < 1e-6, "silent before predelay, rms {before}");
        assert!(after > 1e-4, "tail must arrive after predelay");
    }

    #[test]
    fn hall_stays_bounded_over_a_long_render() {
        let mut r = hall_full_wet(8.0); // max decay
        let ir = impulse_response(&mut r, 10.0);
        assert_finite("long render", &ir);
        assert!(peak(&ir) < 4.0, "FDN must not blow up: peak {}", peak(&ir));
    }

    #[test]
    fn hall_stereo_tails_share_energy_but_decorrelate() {
        let mut r = hall_full_wet(2.0);
        let (l, right) = impulse_response_stereo(&mut r, 1.0);
        let tail = SR as usize / 10..;
        let (tl, tr) = (&l[tail.clone()], &right[tail]);
        let (rl, rr) = (rms(tl), rms(tr));
        assert!(
            (rl / rr).max(rr / rl) < 1.6,
            "channel tail energy must roughly match: {rl} vs {rr}"
        );
        let dot: f64 = tl
            .iter()
            .zip(tr)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        let corr = dot / (f64::from(rl) * f64::from(rr) * tl.len() as f64);
        assert!(corr.abs() < 0.5, "tails must decorrelate, corr {corr:.3}");
    }

    // --- family-wide invariants ---

    #[test]
    fn every_voice_is_finite_bounded_and_silent_in_silent_out() {
        for (voice, def) in VOICES.iter().enumerate() {
            let mut r = prepared(voice);
            set_by(&mut r, "mix", 1.0);
            settle(&mut r, 0.3);
            let x = sine(SR, 220.0, SR as usize);
            let y = process_in_blocks(&mut r, &x, 256);
            assert_finite(def.desc.key, &y);
            assert!(
                peak(&y) < 12.0,
                "{}: output must stay bounded, peak {}",
                def.desc.key,
                peak(&y)
            );

            r.reset();
            let s = silence(SR as usize / 2);
            let y = process_in_blocks(&mut r, &s, 128);
            assert!(rms(&y) == 0.0, "{}: silence in → silence out", def.desc.key);
        }
    }

    #[test]
    fn mix_zero_is_bit_exact_dry_on_every_voice() {
        for (voice, def) in VOICES.iter().enumerate() {
            let mut r = prepared(voice);
            set_by(&mut r, "mix", 0.0);
            settle(&mut r, 1.0); // charge tails, let the mix smoother land
            let x: Vec<f32> = (0..8_192).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
            let mut y = x.clone();
            let mut yr = x.clone();
            r.process(&mut y, &mut yr);
            assert_eq!(x, y, "{}: mix 0 must pass dry (L)", def.desc.key);
            assert_eq!(x, yr, "{}: mix 0 must pass dry (R)", def.desc.key);
        }
    }

    #[test]
    fn every_knob_sweep_stays_finite() {
        // Slam every parameter of every voice min→max mid-note: no NaN, no
        // runaway. (Clicks from stepped knobs are by design; blowups are
        // not.)
        for (voice, def) in VOICES.iter().enumerate() {
            for (i, param) in def.desc.params.iter().enumerate() {
                let mut r = prepared(voice);
                set_by(&mut r, "mix", 1.0);
                let mut x = sine(SR, 330.0, SR as usize);
                let mut xr = x.clone();
                let third = x.len() / 3;
                let (a, rest) = x.split_at_mut(third);
                let (b, c) = rest.split_at_mut(third);
                let (ar, restr) = xr.split_at_mut(third);
                let (br, cr) = restr.split_at_mut(third);
                r.process(a, ar);
                r.set_param(i, 0.0);
                r.process(b, br);
                r.set_param(i, 1.0);
                r.process(c, cr);
                assert_finite(&format!("{} sweep {}", def.desc.key, param.key), &x);
                assert!(
                    peak(&x) < 24.0,
                    "{} sweeping {} must stay bounded, peak {}",
                    def.desc.key,
                    param.key,
                    peak(&x)
                );
            }
        }
    }

    #[test]
    fn pedal_switch_mid_note_stays_finite() {
        let mut r = prepared(0);
        set_by(&mut r, "mix", 0.6);
        let x = sine(SR, 220.0, SR as usize);
        let mut left = x.clone();
        let mut right = x.clone();
        for (i, (bl, br)) in left.chunks_mut(64).zip(right.chunks_mut(64)).enumerate() {
            let v = i % VOICE_COUNT;
            r.select_pedal(v);
            for (p, param) in VOICES[v].desc.params.iter().enumerate() {
                r.set_param(p, param.default_norm()); // control-side re-send
            }
            r.process(bl, br);
        }
        assert_finite("reverb pedal switch", &left);
        assert!(peak(&left) < 8.0);
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            for (voice, def) in VOICES.iter().enumerate() {
                let mut r = Reverb::new();
                r.prepare(sr);
                r.select_pedal(voice);
                for (i, p) in def.desc.params.iter().enumerate() {
                    r.set_param(i, p.default_norm());
                }
                let i = def.desc.param_index("mix").unwrap();
                r.set_param(i, 1.0);
                for chunk in [32usize, 483, 1_024] {
                    let x = sine(sr, 440.0, 4_096);
                    let y = process_in_blocks(&mut r, &x, chunk);
                    assert_finite("reverb multirate", &y);
                }
            }
        }
    }

    // --- per-voice character ---

    #[test]
    fn room_wet_arrives_earlier_than_hall() {
        let onset = |key: &str| {
            let mut r = prepared(voice_of(key));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "predelay", 0.0);
            settle(&mut r, 0.5);
            r.reset();
            let ir = impulse_response(&mut r, 0.3);
            ir.iter().position(|s| s.abs() > 1e-4).unwrap()
        };
        let room = onset("room");
        let hall = onset("hall");
        assert!(
            room * 3 < hall * 2,
            "room first wet must arrive well before hall's: {room} vs {hall}"
        );
    }

    #[test]
    fn plate_keeps_more_top_end_than_hall() {
        // HF ring time, not steady state (a single steady tone through an
        // FDN measures comb interference, not damping): charge with 6 kHz,
        // stop, and listen to how long the top end survives the loop.
        let hf_tail = |key: &str| {
            let mut r = prepared(voice_of(key));
            set_by(&mut r, "decay", 2.0);
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "mod", 0.0); // isolate damping from mod smear
            settle(&mut r, 0.5);
            let mut x = sine(SR, 6_000.0, SR as usize * 8 / 10);
            x.extend(silence(SR as usize / 2));
            let y = process_in_blocks(&mut r, &x, 256);
            let stop = SR as usize * 8 / 10;
            f64::from(rms(&y[stop + SR as usize / 20..stop + SR as usize * 7 / 20]))
        };
        let plate = hf_tail("plate");
        let hall = hf_tail("hall");
        assert!(
            plate > 1.5 * hall,
            "plate's 6 kHz ring must outlast hall's: {plate:.6} vs {hall:.6}"
        );
    }

    #[test]
    fn spring_dwell_drives_harmonics_into_the_tank() {
        let third_harmonic = |dwell: f32| {
            let mut r = prepared(voice_of("spring"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "dwell", dwell);
            settle(&mut r, 0.3);
            let x: Vec<f32> = sine(SR, 400.0, SR as usize)
                .iter()
                .map(|s| s * 0.7)
                .collect();
            let y = process_in_blocks(&mut r, &x, 256);
            level_at(&y[SR as usize / 2..], 1_200.0)
        };
        let clean = third_harmonic(0.0);
        let driven = third_harmonic(1.0);
        assert!(
            driven > 3.0 * clean,
            "dwell must saturate the feed: 3rd harmonic {driven:.6} vs {clean:.6}"
        );
    }

    #[test]
    fn spring_count_changes_the_dispersion() {
        let render = |springs: f32| {
            let mut r = prepared(voice_of("spring"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "springs", springs);
            settle(&mut r, 0.3);
            r.reset();
            impulse_response(&mut r, 0.4)
        };
        let one = render(0.0);
        let three = render(2.0);
        let diff = one
            .iter()
            .zip(&three)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(diff > 1e-3, "1 vs 3 springs must differ, max diff {diff}");
    }

    #[test]
    fn swell_fades_the_reverb_in() {
        let mut r = prepared(voice_of("swell"));
        set_by(&mut r, "mix", 1.0);
        set_by(&mut r, "rise", 700.0);
        settle(&mut r, 0.3);
        let x = sine(SR, 220.0, SR as usize * 3 / 2);
        let y = process_in_blocks(&mut r, &x, 256);
        let early = window_rms(&y, 0.02, 0.15);
        let late = window_rms(&y, 0.9, 1.2);
        assert!(
            early < 0.2 * late,
            "wet must swell in: early {early} vs late {late}"
        );
    }

    #[test]
    fn bloom_feedback_regenerates_the_build() {
        let body = |fb: f32| {
            let mut r = prepared(voice_of("bloom"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "feedback", fb);
            settle(&mut r, 0.3);
            r.reset();
            let ir = impulse_response(&mut r, 1.2);
            f64::from(window_rms(&ir, 0.5, 1.0))
        };
        let dry_loop = body(0.0);
        let regen = body(0.85);
        assert!(
            regen > 2.0 * dry_loop,
            "bloom feedback must thicken the build: {regen:.6} vs {dry_loop:.6}"
        );
    }

    #[test]
    fn cloud_rings_for_ages() {
        let mut r = prepared(voice_of("cloud"));
        set_by(&mut r, "decay", 20.0);
        set_by(&mut r, "mix", 1.0);
        settle(&mut r, 0.3);
        r.reset();
        let ir = impulse_response(&mut r, 6.0);
        assert_finite("cloud ir", &ir);
        let early = window_rms(&ir, 0.3, 0.6);
        let late = window_rms(&ir, 5.5, 6.0);
        assert!(
            late > 0.06 * early,
            "20 s decay must still ring at 6 s: {late} vs {early}"
        );
    }

    #[test]
    fn chorale_vowel_moves_the_formants() {
        // A (F1 800) versus I (F2 2500): the 800/2500 balance must flip.
        let balance = |vowel: f32| {
            let mut r = prepared(voice_of("chorale"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "intensity", 1.0);
            set_by(&mut r, "vowel", vowel);
            settle(&mut r, 0.5);
            // Deterministic wideband probe: an impulse train.
            let mut x = silence(SR as usize * 2);
            for i in (0..x.len()).step_by(487) {
                x[i] = 0.5;
            }
            let y = process_in_blocks(&mut r, &x, 256);
            let tail = &y[SR as usize / 2..];
            level_at(tail, 800.0) / level_at(tail, 2_500.0)
        };
        let a = balance(0.0); // A
        let i = balance(0.5); // I
        assert!(
            a > 3.0 * i,
            "vowel must move energy between formants: A ratio {a:.3} vs I ratio {i:.3}"
        );
    }

    #[test]
    fn shimmer_climbs_an_octave() {
        let octave_tail = |amount: f32| {
            let mut r = prepared(voice_of("shimmer"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "amount", amount);
            settle(&mut r, 0.3);
            let mut x = sine(SR, 440.0, SR as usize * 3 / 10); // 300 ms burst
            x.extend(silence(SR as usize * 2));
            let y = process_in_blocks(&mut r, &x, 256);
            level_at(&y[y.len() - SR as usize..], 880.0)
        };
        let without = octave_tail(0.0);
        let with = octave_tail(1.0);
        assert!(
            with > 5.0 * without.max(1e-9),
            "the tail must grow an octave partial: {with:.7} vs {without:.7}"
        );
    }

    #[test]
    fn shimmer_survives_max_everything() {
        // The regeneration bound: max amount, max decay, a hot input — the
        // wash may drone (that is the sound) but must stay finite/bounded.
        let mut r = prepared(voice_of("shimmer"));
        set_by(&mut r, "mix", 1.0);
        set_by(&mut r, "amount", 1.0);
        set_by(&mut r, "decay", 15.0);
        settle(&mut r, 0.3);
        let x: Vec<f32> = sine(SR, 220.0, SR as usize * 4)
            .iter()
            .map(|s| s * 0.9)
            .collect();
        let y = process_in_blocks(&mut r, &x, 256);
        assert_finite("shimmer max", &y);
        assert!(peak(&y) < 12.0, "bounded drone, peak {}", peak(&y));
    }

    #[test]
    fn magneto_heads_land_on_the_spacing() {
        let render = |heads: f32| {
            let mut r = prepared(voice_of("magneto"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "spacing", 120.0);
            set_by(&mut r, "repeats", 0.0);
            set_by(&mut r, "decay", 0.5);
            set_by(&mut r, "mod", 0.0);
            set_by(&mut r, "heads", heads);
            settle(&mut r, 1.0);
            r.reset();
            impulse_response(&mut r, 0.6)
        };
        let window_peak = |ir: &[f32], at_s: f32| {
            let a = (SR as f32 * (at_s - 0.01)) as usize;
            let b = (SR as f32 * (at_s + 0.01)) as usize;
            peak(&ir[a..b])
        };
        let three = render(2.0); // stepped index 2 = "3"
        assert!(window_peak(&three, 0.12) > 0.3, "head 1 missing");
        assert!(window_peak(&three, 0.24) > 0.25, "head 2 missing");
        assert!(window_peak(&three, 0.36) > 0.2, "head 3 missing");
        let one = render(0.0);
        assert!(window_peak(&one, 0.12) > 0.3, "single head missing");
        assert!(
            window_peak(&one, 0.24) < 0.15,
            "one head must not echo at 240 ms"
        );
    }

    #[test]
    fn magneto_repeats_regenerate() {
        let mut r = prepared(voice_of("magneto"));
        set_by(&mut r, "mix", 1.0);
        set_by(&mut r, "spacing", 100.0);
        set_by(&mut r, "repeats", 0.9);
        set_by(&mut r, "heads", 0.0);
        settle(&mut r, 1.0);
        r.reset();
        let ir = impulse_response(&mut r, 1.5);
        assert_finite("magneto repeats", &ir);
        // Echoes keep landing well past the single head time.
        assert!(window_rms(&ir, 0.9, 1.4) > 1e-3, "repeats must sustain");
    }

    #[test]
    fn nonlinear_gate_cuts_and_reverse_rises() {
        let render = |shape: f32| {
            let mut r = prepared(voice_of("nonlinear"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "decay", 0.5); // the window
            set_by(&mut r, "shape", shape);
            settle(&mut r, 0.3);
            r.reset();
            impulse_response(&mut r, 1.5)
        };
        let gate = render(0.0);
        assert!(
            window_rms(&gate, 0.05, 0.35) > 20.0 * window_rms(&gate, 0.7, 1.2).max(1e-7),
            "gate must cut after the window"
        );
        let reverse = render(1.0);
        assert!(
            window_rms(&reverse, 0.30, 0.42) > 2.0 * window_rms(&reverse, 0.02, 0.15),
            "reverse must rise into the cut"
        );
        // No feedback anywhere: long after the window, silence.
        assert!(window_rms(&reverse, 1.0, 1.5) < 1e-5, "burst must end");
    }

    #[test]
    fn reflections_is_early_only_and_shapes_differ() {
        let render = |shape: f32| {
            let mut r = prepared(voice_of("reflections"));
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "shape", shape);
            settle(&mut r, 0.3);
            r.reset();
            impulse_response(&mut r, 0.6)
        };
        let studio = render(0.0);
        assert!(
            window_rms(&studio, 0.3, 0.6) < 1e-4 * peak(&studio).max(1e-6),
            "no tail: reflections must go silent"
        );
        let dome = render(2.0);
        let diff = studio
            .iter()
            .zip(&dome)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(diff > 1e-3, "tap tables must differ, max diff {diff}");
    }

    #[test]
    fn lowend_shapes_the_lows_both_ways() {
        // Low RT, measured as a tail (steady-state single tones read comb
        // interference): charge with 100 Hz, stop, compare how much low
        // energy the loop still holds.
        let low_tail = |lowend: f32| {
            let mut r = prepared(voice_of("hall"));
            set_by(&mut r, "decay", 3.0);
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "lowend", lowend);
            settle(&mut r, 0.5);
            let mut x = sine(SR, 100.0, SR as usize);
            x.extend(silence(SR as usize));
            let y = process_in_blocks(&mut r, &x, 256);
            let stop = SR as usize;
            f64::from(rms(
                &y[stop + SR as usize * 15 / 100..stop + SR as usize * 6 / 10]
            ))
        };
        let neutral = low_tail(0.5);
        let cut = low_tail(0.0);
        let boost = low_tail(1.0);
        assert!(
            cut < 0.5 * neutral,
            "lowend 0 must shorten the low tail: {cut:.6} vs {neutral:.6}"
        );
        assert!(
            boost > 1.25 * neutral,
            "lowend 1 must thicken the low tail: {boost:.6} vs {neutral:.6}"
        );
    }

    #[test]
    fn mod_knob_wobbles_the_tail() {
        // With mod 0 the tank is LTI: after settling under a periodic
        // input, consecutive periods repeat. With mod up they cannot.
        let block_diff = |depth: f32| {
            let mut r = prepared(voice_of("hall"));
            set_by(&mut r, "decay", 0.3);
            set_by(&mut r, "mix", 1.0);
            set_by(&mut r, "mod", depth);
            settle(&mut r, 0.3);
            let warm = sine(SR, 200.0, SR as usize * 2);
            let _ = process_in_blocks(&mut r, &warm, 512);
            let x = sine(SR, 200.0, 4_800); // exactly 20 periods
            let a = process_in_blocks(&mut r, &x, 4_800);
            let b = process_in_blocks(&mut r, &x, 4_800);
            a.iter()
                .zip(&b)
                .map(|(p, q)| (p - q).abs())
                .fold(0.0f32, f32::max)
        };
        let still = block_diff(0.0);
        let wobbly = block_diff(1.0);
        assert!(
            still < 1e-3,
            "mod 0 steady state must repeat (diff {still})"
        );
        assert!(wobbly > 1e-2, "mod 1 must move the tail (diff {wobbly})");
    }
}
