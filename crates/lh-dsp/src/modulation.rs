//! The modulation family: one slot, eight pedals — chorus, flanger, phaser,
//! tremolo, vibrato, harmonic, rotary, univibe — each with its own faceplate
//! (PRD 001, expanded by PRD 006):
//!
//! - **chorus**: delay line swept 2–14 ms, gentle feedback
//!   (rate/depth/feedback/mix)
//! - **flanger**: delay line swept 1–5 ms, prominent feedback
//!   (rate/depth/feedback/mix)
//! - **phaser**: four first-order allpass stages, cutoff swept 230–2100 Hz
//!   (rate/depth/feedback/mix)
//! - **tremolo**: amplitude LFO, **dB-linear depth** (the ear hears dB, not
//!   linear gain: full depth throbs down to −60 dB, half depth −30 dB —
//!   the M5 linear law barely reached −6 dB at noon, the "is it even on?"
//!   complaint). `wave` picks sine / triangle / chop (slew-limited square),
//!   `spread` sets the R-channel LFO phase 0..180° — **in phase by
//!   default**: both speakers throb together like an amp tremolo; the old
//!   always-half-cycle auto-pan largely cancelled itself in a room
//!   (rate/depth/wave/spread).
//! - **vibrato**: true pitch vibrato — a wet-only swept delay read, both
//!   channels phase-coherent (a pitch bend is one event, not a widener)
//!   (rate/depth).
//! - **harmonic**: brownface-style harmonic tremolo — complementary one-pole
//!   split at 700 Hz, low and high band gain-modulated in **counter-phase**;
//!   the seasick phasey throb without any pitch movement (rate/depth).
//! - **rotary**: a small Leslie — complementary split at 800 Hz, horn and
//!   drum rotors with their own doppler/AM/pan and their own inertia (the
//!   horn spins up in ~1 s, the drum takes ~3 s — flipping `speed` mid-note
//!   is the effect); `balance` mixes drum⇄horn (speed/depth/balance).
//! - **univibe**: four allpass stages at *staggered* corners (the photocell
//!   ladder) swept together by a lamp-like skewed LFO, blended 50/50 with
//!   the dry by construction (rate/depth).
//!
//! All pedals share one LFO and one pair of voices; switching pedals resets
//! the voice state (a brief discontinuity while auditioning, never NaN);
//! continuous params morph smoothly. Param positions route through a
//! per-pedal `Ctl` table (like the reverb family) — faceplates no longer
//! share knob positions.
//!
//! Stereo (M7): chorus/flanger/phaser/harmonic offset the right channel's
//! LFO a quarter cycle (width); tremolo makes that offset a knob; vibrato
//! keeps both channels coherent; rotary pans for real; univibe offsets an
//! eighth of a cycle (a hint of width, the pedal is mono at heart).

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::blocks::smooth::Smoothed;
use crate::blocks::{onepole_hz, onepole_ms};

const CHORUS: usize = 0;
const FLANGER: usize = 1;
const PHASER: usize = 2;
const TREMOLO: usize = 3;
const VIBRATO: usize = 4;
const HARMONIC: usize = 5;
const ROTARY: usize = 6;
const UNIVIBE: usize = 7;

/// Longest modulated delay (chorus max) plus headroom.
const MAX_DELAY_MS: f32 = 20.0;
const PHASER_STAGES: usize = 4;

/// Univibe photocell ladder: four *different* allpass corners (Hz), all
/// swept together — the stagger is what separates a vibe from a phaser.
const UNIVIBE_HZ: [f32; PHASER_STAGES] = [78.0, 210.0, 620.0, 1_750.0];

/// Tremolo full-depth floor: −60 dB (ln(10^-3) = −6.9078), reached at
/// depth 1 on the LFO's bottom. exp(DB_FLOOR_LN · depth · w).
const TREM_FLOOR_LN: f32 = -6.907_755;

/// Harmonic tremolo band split.
const HARMONIC_XOVER_HZ: f32 = 700.0;

/// Rotary voicing: crossover, per-rotor speeds (Hz) slow/fast, inertia
/// (ms — the horn is light, the drum heavy), doppler deviations (ms).
const ROTARY_XOVER_HZ: f32 = 800.0;
const HORN_SLOW_HZ: f32 = 0.85;
const HORN_FAST_HZ: f32 = 6.9;
const DRUM_SLOW_HZ: f32 = 0.66;
const DRUM_FAST_HZ: f32 = 5.4;
const HORN_INERTIA_MS: f32 = 900.0;
const DRUM_INERTIA_MS: f32 = 3_200.0;
const HORN_CENTER_MS: f32 = 1.3;
const HORN_DEV_MS: f32 = 0.85;
const DRUM_CENTER_MS: f32 = 1.1;
const DRUM_DEV_MS: f32 = 0.25;

pub const TREM_WAVES: &[&str] = &["sine", "triangle", "chop"];
pub const ROTARY_SPEEDS: &[&str] = &["slow", "fast"];

const fn rate(default: f32) -> ParamDesc {
    ParamDesc {
        key: "rate",
        name: "Rate",
        unit: "Hz",
        range: Range::Log {
            min: 0.05,
            max: 10.0,
        },
        default,
        smoothing_ms: 80.0,
    }
}

const fn depth(default: f32) -> ParamDesc {
    ParamDesc {
        key: "depth",
        name: "Depth",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 50.0,
    }
}

const FEEDBACK: ParamDesc = ParamDesc {
    key: "feedback",
    name: "Feedback",
    unit: "",
    range: Range::Linear {
        min: 0.0,
        max: 0.85,
    },
    default: 0.25,
    smoothing_ms: 50.0,
};

const MIX: ParamDesc = ParamDesc {
    key: "mix",
    name: "Mix",
    unit: "",
    range: Range::Linear { min: 0.0, max: 1.0 },
    default: 0.5,
    smoothing_ms: 30.0,
};

/// R-channel LFO phase offset, 0 (in phase — amp-style throb) to 1 (half a
/// cycle — full auto-pan).
const SPREAD: ParamDesc = ParamDesc {
    key: "spread",
    name: "Spread",
    unit: "",
    range: Range::Linear { min: 0.0, max: 1.0 },
    default: 0.0,
    smoothing_ms: 60.0,
};

const WAVE: ParamDesc = ParamDesc {
    key: "wave",
    name: "Wave",
    unit: "",
    range: Range::Stepped { labels: TREM_WAVES },
    default: 0.0,
    smoothing_ms: 0.0,
};

/// Global-tempo lock (ADR 014): **Free** = the Rate knob rules; any note
/// division locks the LFO `rate` to the rig's BPM so one cycle spans that note
/// (the standalone session derives it each control tick). Control-side only —
/// a no-op in the audio path, like [`crate::time::delay`]'s `sync`.
const SYNC: ParamDesc = ParamDesc {
    key: "sync",
    name: "Sync",
    unit: "",
    range: Range::Stepped {
        labels: lh_core::tempo::SYNC_DIVISIONS,
    },
    default: 0.0, // Free
    smoothing_ms: 0.0,
};

const SPEED: ParamDesc = ParamDesc {
    key: "speed",
    name: "Speed",
    unit: "",
    range: Range::Stepped {
        labels: ROTARY_SPEEDS,
    },
    default: 0.0,
    smoothing_ms: 0.0,
};

/// Rotary drum⇄horn balance (equal-power crossfade, 0.5 = both).
const BALANCE: ParamDesc = ParamDesc {
    key: "balance",
    name: "Balance",
    unit: "",
    range: Range::Linear { min: 0.0, max: 1.0 },
    default: 0.5,
    smoothing_ms: 60.0,
};

// chorus/flanger/phaser keep the v2 keys, ranges, and defaults, so sparse
// v2 presets migrate without pinning. Tremolo keeps `rate`/`depth` leading
// (the v2 fold writes those keys) and appends its new knobs.
static CHORUS_PARAMS: [ParamDesc; 4] = [rate(0.8), depth(0.5), FEEDBACK, MIX];
static CHORUS_DESC: EffectDesc = EffectDesc {
    key: "chorus",
    name: "Chorus",
    params: &CHORUS_PARAMS,
};

static FLANGER_PARAMS: [ParamDesc; 4] = [rate(0.8), depth(0.5), FEEDBACK, MIX];
static FLANGER_DESC: EffectDesc = EffectDesc {
    key: "flanger",
    name: "Flanger",
    params: &FLANGER_PARAMS,
};

static PHASER_PARAMS: [ParamDesc; 4] = [rate(0.8), depth(0.5), FEEDBACK, MIX];
static PHASER_DESC: EffectDesc = EffectDesc {
    key: "phaser",
    name: "Phaser",
    params: &PHASER_PARAMS,
};

static TREMOLO_PARAMS: [ParamDesc; 5] = [rate(5.0), depth(0.65), WAVE, SPREAD, SYNC];
static TREMOLO_DESC: EffectDesc = EffectDesc {
    key: "tremolo",
    name: "Tremolo",
    params: &TREMOLO_PARAMS,
};

static VIBRATO_PARAMS: [ParamDesc; 2] = [rate(5.0), depth(0.5)];
static VIBRATO_DESC: EffectDesc = EffectDesc {
    key: "vibrato",
    name: "Vibrato",
    params: &VIBRATO_PARAMS,
};

static HARMONIC_PARAMS: [ParamDesc; 2] = [rate(3.0), depth(0.7)];
static HARMONIC_DESC: EffectDesc = EffectDesc {
    key: "harmonic",
    name: "Harmonic",
    params: &HARMONIC_PARAMS,
};

static ROTARY_PARAMS: [ParamDesc; 3] = [SPEED, depth(0.7), BALANCE];
static ROTARY_DESC: EffectDesc = EffectDesc {
    key: "rotary",
    name: "Rotary",
    params: &ROTARY_PARAMS,
};

static UNIVIBE_PARAMS: [ParamDesc; 2] = [rate(1.2), depth(0.7)];
static UNIVIBE_DESC: EffectDesc = EffectDesc {
    key: "univibe",
    name: "Univibe",
    params: &UNIVIBE_PARAMS,
};

/// The modulation family, in menu order. The first four are pinned to
/// `lh_core::preset::MOD_PEDALS` (the v2 migration) by a test below;
/// everything after is append-only post-v2.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "mod",
    name: "Modulation",
    pedals: &[
        &CHORUS_DESC,
        &FLANGER_DESC,
        &PHASER_DESC,
        &TREMOLO_DESC,
        &VIBRATO_DESC,
        &HARMONIC_DESC,
        &ROTARY_DESC,
        &UNIVIBE_DESC,
    ],
};

/// Which engine control a pedal's param position drives (PRD 006 — the
/// faceplates stopped sharing knob positions when tremolo grew wave/spread
/// and rotary led with a stepped speed).
#[derive(Clone, Copy)]
enum Ctl {
    Rate,
    Depth,
    Feedback,
    Mix,
    Wave,
    Spread,
    Speed,
    Balance,
    /// Global-tempo lock selector (control-side only; the session derives the
    /// LFO rate from the rig BPM).
    Sync,
}

/// Param→control routing, aligned with [`FAMILY`]`.pedals`.
static CONTROLS: [&[Ctl]; 8] = [
    &[Ctl::Rate, Ctl::Depth, Ctl::Feedback, Ctl::Mix], // chorus
    &[Ctl::Rate, Ctl::Depth, Ctl::Feedback, Ctl::Mix], // flanger
    &[Ctl::Rate, Ctl::Depth, Ctl::Feedback, Ctl::Mix], // phaser
    &[Ctl::Rate, Ctl::Depth, Ctl::Wave, Ctl::Spread, Ctl::Sync], // tremolo
    &[Ctl::Rate, Ctl::Depth],                          // vibrato
    &[Ctl::Rate, Ctl::Depth],                          // harmonic
    &[Ctl::Speed, Ctl::Depth, Ctl::Balance],           // rotary
    &[Ctl::Rate, Ctl::Depth],                          // univibe
];

/// One channel's voice: delay line (chorus/flanger/vibrato, rotary rotors),
/// allpass chain (phaser/univibe), crossover + feedback memory. Two of
/// these make the stereo pair.
struct Voice {
    buf: Vec<f32>,
    write: usize,
    ap_x1: [f32; PHASER_STAGES],
    ap_y1: [f32; PHASER_STAGES],
    fb: f32,
    xover_lp: f32,
}

impl Voice {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            write: 0,
            ap_x1: [0.0; PHASER_STAGES],
            ap_y1: [0.0; PHASER_STAGES],
            fb: 0.0,
            xover_lp: 0.0,
        }
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.ap_x1 = [0.0; PHASER_STAGES];
        self.ap_y1 = [0.0; PHASER_STAGES];
        self.fb = 0.0;
        self.xover_lp = 0.0;
    }

    /// Interpolated read `delay_smp` samples behind the write head.
    #[inline]
    fn read_delayed(&self, delay_smp: f32) -> f32 {
        let len = self.buf.len() as f32;
        let rp = self.write as f32 - delay_smp + len;
        let i0 = rp as usize;
        let frac = rp - i0 as f32;
        let a = self.buf[i0 % self.buf.len()];
        let b = self.buf[(i0 + 1) % self.buf.len()];
        a + frac * (b - a)
    }

    #[inline]
    fn push(&mut self, value: f32) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.buf.len();
    }

    /// One wet sample of the delay/allpass pedals driven by this channel's
    /// LFO value. Tremolo and rotary live in `process` (gain/pan laws, no
    /// per-voice symmetry).
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        mode: usize,
        x: f32,
        lfo: f32,
        depth: f32,
        feedback: f32,
        ms: f32,
        sample_rate: f32,
        xover_coeff: f32,
    ) -> f32 {
        match mode {
            CHORUS | FLANGER | VIBRATO => {
                let delay_ms = match mode {
                    CHORUS => 8.0 + 6.0 * depth * lfo, // 2..14 ms
                    // Pitch bend wants symmetric travel around its center.
                    VIBRATO => 5.5 + 4.5 * depth * lfo, // 1..10 ms
                    _ => 1.0 + 4.0 * depth * (0.5 + 0.5 * lfo), // 1..5 ms
                };
                let delay_smp = (delay_ms * ms).clamp(1.0, (self.buf.len() - 2) as f32);
                let tap = self.read_delayed(delay_smp);
                self.push(x + feedback * tap);
                tap
            }
            PHASER | UNIVIBE => {
                let mut y = x + feedback * self.fb;
                if mode == PHASER {
                    // Sweep the allpass corner geometrically around 700 Hz.
                    let fc = 700.0 * 3f32.powf(lfo * depth);
                    let t = (std::f32::consts::PI * fc / sample_rate).tan();
                    let a = (1.0 - t) / (1.0 + t);
                    for stage in 0..PHASER_STAGES {
                        let out = -a * y + self.ap_x1[stage] + a * self.ap_y1[stage];
                        self.ap_x1[stage] = y;
                        self.ap_y1[stage] = out;
                        y = out;
                    }
                } else {
                    // Univibe: staggered corners swept together (±1.2 oct).
                    let sweep = 2f32.powf(1.2 * depth * lfo);
                    for ((&hz, x1), y1) in
                        UNIVIBE_HZ.iter().zip(&mut self.ap_x1).zip(&mut self.ap_y1)
                    {
                        let t = (std::f32::consts::PI * hz * sweep / sample_rate).tan();
                        let a = (1.0 - t) / (1.0 + t);
                        let out = -a * y + *x1 + a * *y1;
                        *x1 = y;
                        *y1 = out;
                        y = out;
                    }
                }
                self.fb = y;
                y
            }
            HARMONIC => {
                // Complementary split; the bands throb in counter-phase, so
                // the sum is exactly `x` at depth 0 (and the timbre, not the
                // level, is what wobbles at any depth).
                self.xover_lp += xover_coeff * (x - self.xover_lp);
                if self.xover_lp.abs() < 1e-20 {
                    self.xover_lp = 0.0;
                }
                if depth < 1e-7 {
                    // Bit-exact off (the split re-sum rounds): anything
                    // below −140 dB of modulation is "off".
                    return x;
                }
                let low = self.xover_lp;
                let high = x - low;
                let w = 0.5 + 0.5 * lfo;
                low * (1.0 - depth * w) + high * (1.0 - depth * (1.0 - w))
            }
            _ => x, // tremolo/rotary handled in process(); unreachable here
        }
    }
}

pub struct Modulation {
    sample_rate: f32,
    mode: usize,
    rate: Smoothed,
    depth: Smoothed,
    feedback: Smoothed,
    mix: Smoothed,
    spread: Smoothed,
    balance: Smoothed,
    wave: usize,
    phase: f32,
    voices: [Voice; 2],
    // tremolo: slewed per-channel gain (declicks the chop wave)
    trem_g: [f32; 2],
    trem_slew: f32,
    // crossover coefficients (rebuilt in prepare)
    harmonic_coeff: f32,
    rotary_coeff: f32,
    // rotary: rotor speeds with inertia, phases, and the mono-side split
    horn_rate: Smoothed,
    drum_rate: Smoothed,
    horn_phase: f32,
    drum_phase: f32,
    rot_lp: f32,
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulation {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            mode: CHORUS,
            rate: Smoothed::new(CHORUS_PARAMS[0].default),
            depth: Smoothed::new(CHORUS_PARAMS[1].default),
            feedback: Smoothed::new(FEEDBACK.default),
            mix: Smoothed::new(MIX.default),
            spread: Smoothed::new(SPREAD.default),
            balance: Smoothed::new(BALANCE.default),
            wave: 0,
            phase: 0.0,
            voices: [Voice::new(), Voice::new()],
            trem_g: [1.0; 2],
            trem_slew: 1.0,
            harmonic_coeff: 0.1,
            rotary_coeff: 0.1,
            horn_rate: Smoothed::new(HORN_SLOW_HZ),
            drum_rate: Smoothed::new(DRUM_SLOW_HZ),
            horn_phase: 0.0,
            drum_phase: 0.0,
            rot_lp: 0.0,
        }
    }

    fn clear_voices(&mut self) {
        for voice in &mut self.voices {
            voice.clear();
        }
        self.trem_g = [1.0; 2];
        self.rot_lp = 0.0;
    }

    /// Tremolo LFO shape: `w` in 0..1 (1 = full dip).
    #[inline]
    fn trem_wave(&self, phase: f32) -> f32 {
        match self.wave {
            1 => {
                // triangle: 0 → 1 → 0 over the cycle
                let p = phase / std::f32::consts::TAU;
                2.0 * (p - (p + 0.5).floor()).abs()
            }
            2 => {
                if phase.sin() >= 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 0.5 + 0.5 * phase.sin(),
        }
    }
}

impl Effect for Modulation {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.mode
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal != self.mode && pedal < FAMILY.pedals.len() {
            self.mode = pedal;
            self.clear_voices();
            // Rotors start from rest at the slow speed; the incoming
            // pedal's `speed` value (re-sent by the control side) then
            // glides there — an authentic spin-up on arrival.
            self.horn_rate.set_target(HORN_SLOW_HZ);
            self.drum_rate.set_target(DRUM_SLOW_HZ);
            self.horn_rate.snap_to_target();
            self.drum_rate.snap_to_target();
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        for voice in &mut self.voices {
            voice.buf = vec![0.0; (MAX_DELAY_MS * 1e-3 * self.sample_rate) as usize + 4];
        }
        // Smoothing times mirror the faceplate descs; the rotor "smoothers"
        // are the rotary inertia constants.
        for (smoothed, ms) in [
            (&mut self.rate, 80.0),
            (&mut self.depth, 50.0),
            (&mut self.feedback, 50.0),
            (&mut self.mix, 30.0),
            (&mut self.spread, 60.0),
            (&mut self.balance, 60.0),
            (&mut self.horn_rate, HORN_INERTIA_MS),
            (&mut self.drum_rate, DRUM_INERTIA_MS),
        ] {
            smoothed.configure(ms, sample_rate);
            smoothed.snap_to_target();
        }
        self.trem_slew = onepole_ms(1.2, sample_rate);
        self.harmonic_coeff = onepole_hz(HARMONIC_XOVER_HZ, self.sample_rate);
        self.rotary_coeff = onepole_hz(ROTARY_XOVER_HZ, self.sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.horn_phase = 0.0;
        self.drum_phase = 0.0;
        self.clear_voices();
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let (Some(ctl), Some(param)) = (
            CONTROLS[self.mode].get(index),
            FAMILY.pedals[self.mode].params.get(index),
        ) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Rate => self.rate.set_target(real),
            Ctl::Depth => self.depth.set_target(real),
            Ctl::Feedback => self.feedback.set_target(real),
            Ctl::Mix => self.mix.set_target(real),
            Ctl::Wave => self.wave = real as usize,
            Ctl::Spread => self.spread.set_target(real),
            Ctl::Speed => {
                let fast = real >= 0.5;
                self.horn_rate
                    .set_target(if fast { HORN_FAST_HZ } else { HORN_SLOW_HZ });
                self.drum_rate
                    .set_target(if fast { DRUM_FAST_HZ } else { DRUM_SLOW_HZ });
            }
            Ctl::Balance => self.balance.set_target(real),
            Ctl::Sync => {} // control-side only (the session derives rate)
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.voices[0].buf.is_empty() {
            return; // prepare() not called yet
        }
        let ms = self.sample_rate * 1e-3;

        if self.mode == ROTARY {
            // The rotary is its own animal: mono into the cabinet, stereo
            // out of the mics. Two rotors with independent inertia; the
            // horn dopplers/pans hard, the drum wallows underneath.
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let depth = self.depth.tick();
                let balance = self.balance.tick();
                let horn_hz = self.horn_rate.tick();
                let drum_hz = self.drum_rate.tick();
                self.horn_phase += std::f32::consts::TAU * horn_hz / self.sample_rate;
                if self.horn_phase >= std::f32::consts::TAU {
                    self.horn_phase -= std::f32::consts::TAU;
                }
                self.drum_phase += std::f32::consts::TAU * drum_hz / self.sample_rate;
                if self.drum_phase >= std::f32::consts::TAU {
                    self.drum_phase -= std::f32::consts::TAU;
                }

                let x = 0.5 * (*l + *r);
                self.rot_lp += self.rotary_coeff * (x - self.rot_lp);
                if self.rot_lp.abs() < 1e-20 {
                    self.rot_lp = 0.0;
                }
                let low = self.rot_lp;
                let high = x - low;

                // Horn: doppler read + AM + pan, all off the horn phase.
                let (h_sin, h_cos) = self.horn_phase.sin_cos();
                self.voices[0].push(high);
                let horn_tap = self.voices[0]
                    .read_delayed((HORN_CENTER_MS + HORN_DEV_MS * depth * h_sin) * ms);
                let horn = horn_tap * (1.0 - 0.3 * depth * (0.5 + 0.5 * h_cos));
                let horn_pan = 0.8 * depth * h_cos;

                // Drum: slower, subtler, narrower.
                let (d_sin, d_cos) = self.drum_phase.sin_cos();
                self.voices[1].push(low);
                let drum_tap = self.voices[1]
                    .read_delayed((DRUM_CENTER_MS + DRUM_DEV_MS * depth * d_sin) * ms);
                let drum = drum_tap * (1.0 - 0.15 * depth * (0.5 + 0.5 * d_cos));
                let drum_pan = 0.3 * depth * d_cos;

                // Equal-power drum⇄horn balance, linear pan per rotor.
                let arg = balance * std::f32::consts::FRAC_PI_2;
                let (horn_g, drum_g) = (arg.sin(), arg.cos());
                *l = drum_g * drum * (1.0 + drum_pan) + horn_g * horn * (1.0 + horn_pan);
                *r = drum_g * drum * (1.0 - drum_pan) + horn_g * horn * (1.0 - horn_pan);
            }
            return;
        }

        // R-channel LFO offset: a knob for tremolo, quadrature width for
        // chorus/flanger/phaser/harmonic, coherent for vibrato (a pitch
        // bend is one event), an eighth-cycle hint for univibe.
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let rate = self.rate.tick();
            let depth = self.depth.tick();
            let feedback = self.feedback.tick();
            let mix = self.mix.tick();
            let spread = self.spread.tick();
            let offset = match self.mode {
                TREMOLO => spread * std::f32::consts::PI,
                VIBRATO => 0.0,
                UNIVIBE => std::f32::consts::FRAC_PI_4,
                _ => std::f32::consts::FRAC_PI_2,
            };

            self.phase += std::f32::consts::TAU * rate / self.sample_rate;
            if self.phase >= std::f32::consts::TAU {
                self.phase -= std::f32::consts::TAU;
            }
            let phase_r = self.phase + offset;

            let (dry_l, dry_r) = (*l, *r);
            if self.mode == TREMOLO {
                // dB-linear depth: the dip bottoms at −60 dB × depth, so
                // half depth is −30 dB — unmissable — instead of the old
                // linear law's −6 dB. The slew declicks the chop wave (and
                // snaps once settled, keeping depth 0 bit-exact).
                for (ch, (phase, dry)) in [(self.phase, dry_l), (phase_r, dry_r)]
                    .into_iter()
                    .enumerate()
                {
                    let w = self.trem_wave(phase);
                    let target = (TREM_FLOOR_LN * depth * w).exp();
                    self.trem_g[ch] += self.trem_slew * (target - self.trem_g[ch]);
                    if (target - self.trem_g[ch]).abs() < 1e-6 {
                        self.trem_g[ch] = target;
                    }
                    let out = dry * self.trem_g[ch];
                    if ch == 0 {
                        *l = out;
                    } else {
                        *r = out;
                    }
                }
                continue;
            }

            let lfo_l = self.phase.sin();
            let lfo_r = phase_r.sin();
            // Univibe throb: the lamp brightens slowly, dims fast — skew
            // the sine toward its bottom before sweeping.
            let (lfo_l, lfo_r) = if self.mode == UNIVIBE {
                let skew = |s: f32| {
                    let w = (0.5 + 0.5 * s).powf(1.6);
                    2.0 * w - 1.0
                };
                (skew(lfo_l), skew(lfo_r))
            } else {
                (lfo_l, lfo_r)
            };

            let wet_l = self.voices[0].step(
                self.mode,
                dry_l,
                lfo_l,
                depth,
                feedback,
                ms,
                self.sample_rate,
                self.harmonic_coeff,
            );
            let wet_r = self.voices[1].step(
                self.mode,
                dry_r,
                lfo_r,
                depth,
                feedback,
                ms,
                self.sample_rate,
                self.harmonic_coeff,
            );
            match self.mode {
                // Wet-only pedals: the knobs, not a mix, set the strength.
                VIBRATO | HARMONIC => {
                    *l = wet_l;
                    *r = wet_r;
                }
                // The vibe sound *is* the fixed half-and-half blend.
                UNIVIBE => {
                    *l = 0.5 * (dry_l + wet_l);
                    *r = 0.5 * (dry_r + wet_r);
                }
                _ => {
                    *l = dry_l + mix * (wet_l - dry_l);
                    *r = dry_r + mix * (wet_r - dry_r);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, process_stereo_in_blocks, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared(mode: usize) -> Modulation {
        let mut m = Modulation::new();
        m.prepare(SR);
        m.select_pedal(mode);
        for (i, p) in FAMILY.pedals[mode].params.iter().enumerate() {
            m.set_param(i, p.default_norm());
        }
        m
    }

    /// Set a param by key with a real value on the active pedal.
    fn set_by(m: &mut Modulation, key: &str, real: f32) {
        let desc = FAMILY.pedals[m.pedal_index()];
        let i = desc.param_index(key).unwrap();
        m.set_param(i, desc.params[i].range.to_norm(real));
    }

    /// `(index, name)` pedal iterator for the character loops.
    fn pedals() -> impl Iterator<Item = (usize, &'static str)> {
        FAMILY.pedals.iter().enumerate().map(|(i, p)| (i, p.key))
    }

    /// Windowed RMS series (25 ms windows) of the second half of `y`.
    fn pump_profile(y: &[f32]) -> (f32, f32) {
        let win = SR as usize / 40;
        let rms_per: Vec<f32> = y[SR as usize / 2..].chunks(win).map(rms).collect();
        let max = rms_per.iter().fold(0.0f32, |m, v| m.max(*v));
        let min = rms_per.iter().fold(f32::INFINITY, |m, v| m.min(*v));
        (min, max)
    }

    /// Projection of `x` onto `freq` (normalized magnitude).
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

    #[test]
    fn registry_is_consistent() {
        // The v2 migration's index map covers exactly the first four.
        let keys: Vec<&str> = FAMILY.pedals.iter().map(|p| p.key).collect();
        assert_eq!(&keys[..4], &lh_core::preset::MOD_PEDALS);
        assert_eq!(FAMILY.pedals.len(), CONTROLS.len());
        for (pedal, controls) in FAMILY.pedals.iter().zip(&CONTROLS) {
            assert_eq!(pedal.params.len(), controls.len(), "{}", pedal.key);
        }
        for (i, a) in FAMILY.pedals.iter().enumerate() {
            for b in &FAMILY.pedals[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
        // Faceplates: tremolo grew wave/spread (rate/depth keys keep their
        // v2 fold positions); the new pedals wear their own faces.
        let captions =
            |i: usize| -> Vec<&str> { FAMILY.pedals[i].params.iter().map(|p| p.name).collect() };
        assert_eq!(
            captions(TREMOLO),
            ["Rate", "Depth", "Wave", "Spread", "Sync"]
        );
        assert_eq!(captions(VIBRATO), ["Rate", "Depth"]);
        assert_eq!(captions(HARMONIC), ["Rate", "Depth"]);
        assert_eq!(captions(ROTARY), ["Speed", "Depth", "Balance"]);
        assert_eq!(captions(UNIVIBE), ["Rate", "Depth"]);
        for i in [CHORUS, FLANGER, PHASER] {
            assert_eq!(captions(i), ["Rate", "Depth", "Feedback", "Mix"]);
        }
        // Spread ships in phase: an amp tremolo, not an auto-panner.
        let spread = &TREMOLO_DESC.params[TREMOLO_DESC.param_index("spread").unwrap()];
        assert_eq!(spread.default, 0.0);
    }

    #[test]
    fn all_modes_render_finite_bounded_audio() {
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            if FAMILY.pedals[mode].param_index("feedback").is_some() {
                set_by(&mut m, "feedback", 0.85);
            }
            let x = sine(SR, 220.0, SR as usize);
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
            assert_finite(name, &l);
            assert_finite(name, &r);
            for (label, y) in [("L", &l), ("R", &r)] {
                let peak = y.iter().fold(0.0f32, |p, s| p.max(s.abs()));
                assert!(peak < 4.0, "{name} {label} runs away: peak {peak}");
            }
        }
    }

    #[test]
    fn width_pedals_decorrelate_and_vibrato_stays_coherent() {
        // Quadrature (or panned) pedals must differ across channels; the
        // pitch pedals must NOT — a bend is one event on both speakers.
        let max_diff = |mode: usize, setup: &dyn Fn(&mut Modulation)| {
            let mut m = prepared(mode);
            setup(&mut m);
            let x = sine(SR, 220.0, SR as usize / 2);
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
            l.iter()
                .zip(&r)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max)
        };
        for (mode, name) in [(CHORUS, "chorus"), (FLANGER, "flanger"), (PHASER, "phaser")] {
            let d = max_diff(mode, &|m| {
                set_by(m, "rate", 2.0);
                set_by(m, "depth", 1.0);
                set_by(m, "mix", 1.0);
            });
            assert!(d > 0.05, "{name} must be wide, max |L-R| = {d}");
        }
        let d = max_diff(HARMONIC, &|m| {
            set_by(m, "rate", 2.0);
            set_by(m, "depth", 1.0);
        });
        assert!(d > 0.05, "harmonic must be wide, max |L-R| = {d}");
        let d = max_diff(UNIVIBE, &|m| {
            set_by(m, "rate", 2.0);
            set_by(m, "depth", 1.0);
        });
        assert!(d > 0.02, "univibe needs its hint of width, max |L-R| = {d}");
        let d = max_diff(ROTARY, &|m| {
            set_by(m, "speed", 1.0);
            set_by(m, "depth", 1.0);
        });
        assert!(d > 0.05, "rotary must pan, max |L-R| = {d}");

        // Coherent pedals: identical inputs must yield identical channels.
        for (mode, name) in [(VIBRATO, "vibrato"), (TREMOLO, "tremolo (spread 0)")] {
            let d = max_diff(mode, &|m| {
                set_by(m, "rate", 5.0);
                set_by(m, "depth", 1.0);
            });
            assert!(d == 0.0, "{name} must stay coherent, max |L-R| = {d}");
        }
    }

    #[test]
    fn dry_positions_are_bit_exact() {
        // Each pedal's "off" knob must pass the dry signal untouched.
        // vibrato/rotary/univibe have no dry position by design (fixed
        // delay / cabinet / fixed blend).
        let recipes: [(usize, &str, &str); 5] = [
            (CHORUS, "mix", "chorus"),
            (FLANGER, "mix", "flanger"),
            (PHASER, "mix", "phaser"),
            (TREMOLO, "depth", "tremolo"),
            (HARMONIC, "depth", "harmonic"),
        ];
        for (mode, knob, name) in recipes {
            let mut m = prepared(mode);
            set_by(&mut m, knob, 0.0);
            let warm = sine(SR, 220.0, SR as usize);
            let _ = process_stereo_in_blocks(&mut m, &warm, 512);
            let x = sine(SR, 220.0, 8_192);
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 512);
            assert_eq!(x, l, "{name} L must pass dry");
            assert_eq!(x, r, "{name} R must pass dry");
        }
    }

    #[test]
    fn tremolo_throbs_hard_in_phase() {
        // The fix for "is it even on?": dB-linear depth and an in-phase
        // default. Full depth must dive toward silence on BOTH channels at
        // once — the mono sum pumps just as hard (no auto-pan cancelling).
        let mut m = prepared(TREMOLO);
        set_by(&mut m, "rate", 4.0);
        set_by(&mut m, "depth", 1.0);
        let x = sine(SR, 220.0, SR as usize);
        let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
        for (label, y) in [("L", &l), ("R", &r)] {
            let (min, max) = pump_profile(y);
            assert!(
                min < 0.05 * max,
                "tremolo {label} must throb: {min} vs {max}"
            );
        }
        let sum: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a + b).collect();
        let (min, max) = pump_profile(&sum);
        assert!(
            min < 0.05 * max,
            "in phase: the room sum must throb too ({min} vs {max})"
        );

        // And the default depth is unmissable, not polite: > 20 dB of pump.
        let mut m = prepared(TREMOLO);
        set_by(&mut m, "rate", 4.0);
        let (l, _) = process_stereo_in_blocks(&mut m, &x, 64);
        let (min, max) = pump_profile(&l);
        assert!(
            min < 0.1 * max,
            "default depth must be obvious: {min} vs {max}"
        );
    }

    #[test]
    fn tremolo_spread_turns_the_throb_into_ping_pong() {
        // At full spread the channels gate in alternation: each side still
        // pumps hard, and their envelopes anti-correlate (when L dips, R
        // blooms). With the dB depth law the sum is NOT conserved — this is
        // a hard ping-pong, not a constant-power panner, by design.
        let mut m = prepared(TREMOLO);
        set_by(&mut m, "rate", 4.0);
        set_by(&mut m, "depth", 1.0);
        set_by(&mut m, "spread", 1.0);
        let x = sine(SR, 220.0, SR as usize);
        let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
        let (l_min, l_max) = pump_profile(&l);
        assert!(l_min < 0.05 * l_max, "each side still pumps");
        let win = SR as usize / 40;
        let env = |y: &[f32]| -> Vec<f32> { y[SR as usize / 2..].chunks(win).map(rms).collect() };
        let (el, er) = (env(&l), env(&r));
        let n = el.len() as f64;
        let (ml, mr) = (
            el.iter().map(|v| f64::from(*v)).sum::<f64>() / n,
            er.iter().map(|v| f64::from(*v)).sum::<f64>() / n,
        );
        let (mut cov, mut vl, mut vr) = (0.0, 0.0, 0.0);
        for (a, b) in el.iter().zip(&er) {
            let (da, db) = (f64::from(*a) - ml, f64::from(*b) - mr);
            cov += da * db;
            vl += da * da;
            vr += db * db;
        }
        let corr = cov / (vl.sqrt() * vr.sqrt()).max(1e-12);
        assert!(
            corr < -0.5,
            "spread 1: channel envelopes must alternate, correlation {corr:.3}"
        );
    }

    #[test]
    fn tremolo_chop_gates_without_clicks() {
        let mut m = prepared(TREMOLO);
        set_by(&mut m, "rate", 6.0);
        set_by(&mut m, "depth", 1.0);
        set_by(&mut m, "wave", 2.0); // chop
        let x = sine(SR, 220.0, SR as usize);
        let (l, _) = process_stereo_in_blocks(&mut m, &x, 64);
        assert_finite("chop", &l);
        let (min, max) = pump_profile(&l);
        assert!(min < 0.02 * max, "chop must gate: {min} vs {max}");
        // Slew keeps edges bounded: no sample step larger than the carrier
        // could produce through a ~1 ms ramp.
        let max_step = l
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.4, "chop edges must be slewed, step {max_step}");
    }

    #[test]
    fn vibrato_bends_pitch_not_level() {
        // Strong FM smears the carrier into sidebands: the 440 Hz line
        // collapses while total energy stays put.
        let mut m = prepared(VIBRATO);
        set_by(&mut m, "rate", 5.0);
        set_by(&mut m, "depth", 1.0);
        let x = sine(SR, 440.0, SR as usize * 2);
        let (l, _) = process_stereo_in_blocks(&mut m, &x, 64);
        let tail_in = &x[SR as usize / 2..];
        let tail_out = &l[SR as usize / 2..];
        let carrier_in = level_at(tail_in, 440.0);
        let carrier_out = level_at(tail_out, 440.0);
        assert!(
            carrier_out < 0.5 * carrier_in,
            "FM must smear the carrier: {carrier_out:.5} vs {carrier_in:.5}"
        );
        let (ri, ro) = (rms(tail_in), rms(tail_out));
        assert!(
            (ro / ri) > 0.8 && (ro / ri) < 1.25,
            "vibrato must not pump levels: {ro} vs {ri}"
        );
    }

    #[test]
    fn harmonic_bands_move_in_counter_phase() {
        // Two probes, one per band: when the lows dip the highs bloom.
        let mut m = prepared(HARMONIC);
        set_by(&mut m, "rate", 2.0);
        set_by(&mut m, "depth", 1.0);
        let x: Vec<f32> = (0..SR as usize * 2)
            .map(|n| {
                let t = n as f32 / SR as f32;
                0.4 * (std::f32::consts::TAU * 150.0 * t).sin()
                    + 0.4 * (std::f32::consts::TAU * 3_000.0 * t).sin()
            })
            .collect();
        let (l, _) = process_stereo_in_blocks(&mut m, &x, 64);
        let win = SR as usize / 40; // 25 ms ≪ the 500 ms LFO cycle
        let series: Vec<(f64, f64)> = l[SR as usize / 2..]
            .chunks(win)
            .map(|w| (level_at(w, 150.0), level_at(w, 3_000.0)))
            .collect();
        let n = series.len() as f64;
        let (mlo, mhi) = series
            .iter()
            .fold((0.0, 0.0), |(a, b), (lo, hi)| (a + lo / n, b + hi / n));
        let (mut cov, mut vlo, mut vhi) = (0.0, 0.0, 0.0);
        for (lo, hi) in &series {
            cov += (lo - mlo) * (hi - mhi);
            vlo += (lo - mlo) * (lo - mlo);
            vhi += (hi - mhi) * (hi - mhi);
        }
        let corr = cov / (vlo.sqrt() * vhi.sqrt()).max(1e-12);
        assert!(
            corr < -0.3,
            "bands must throb in counter-phase, correlation {corr:.3}"
        );
    }

    #[test]
    fn rotary_spins_up_with_inertia() {
        // Flip to fast from rest: the pan wobble must audibly accelerate
        // between the first half-second and three seconds in.
        let mut m = prepared(ROTARY);
        set_by(&mut m, "depth", 1.0);
        set_by(&mut m, "balance", 1.0); // horn only: a clean pan signal
        set_by(&mut m, "speed", 1.0); // fast
        let x = sine(SR, 660.0, SR as usize * 3);
        let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
        // Envelope of L−R per 10 ms window; count its direction flips.
        let win = SR as usize / 100;
        let flips = |from: usize, to: usize| {
            let env: Vec<f32> = l[from..to]
                .iter()
                .zip(&r[from..to])
                .map(|(a, b)| a - b)
                .collect::<Vec<_>>()
                .chunks(win)
                .map(rms)
                .collect();
            let mut count = 0;
            for w in env.windows(3) {
                if (w[1] > w[0]) != (w[2] > w[1]) {
                    count += 1;
                }
            }
            count
        };
        let early = flips(0, SR as usize / 2);
        let late = flips(SR as usize * 5 / 2, SR as usize * 3);
        assert!(
            late * 2 >= early * 3,
            "horn must accelerate: early flips {early}, late flips {late}"
        );
    }

    #[test]
    fn univibe_is_not_the_phaser() {
        // Staggered corners: same knobs, audibly different machine.
        let render = |mode: usize| {
            let mut m = prepared(mode);
            set_by(&mut m, "rate", 1.2);
            set_by(&mut m, "depth", 0.7);
            if FAMILY.pedals[mode].param_index("mix").is_some() {
                set_by(&mut m, "mix", 0.5);
                set_by(&mut m, "feedback", 0.0);
            }
            let x = sine(SR, 330.0, SR as usize);
            process_stereo_in_blocks(&mut m, &x, 64).0
        };
        let vibe = render(UNIVIBE);
        let phaser = render(PHASER);
        let diff = vibe
            .iter()
            .zip(&phaser)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            diff > 0.05,
            "univibe must not collapse into the phaser: {diff}"
        );
    }

    #[test]
    fn output_is_time_varying() {
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            if FAMILY.pedals[mode].param_index("rate").is_some() {
                set_by(&mut m, "rate", 4.0);
            }
            set_by(&mut m, "depth", 1.0);
            if FAMILY.pedals[mode].param_index("mix").is_some() {
                set_by(&mut m, "mix", 1.0);
            }
            let x = sine(SR, 220.0, 4_800);
            let (first, _) = process_stereo_in_blocks(&mut m, &x, 4_800);
            let (second, _) = process_stereo_in_blocks(&mut m, &x, 4_800);
            assert_ne!(first, second, "{name} must modulate over time");
        }
    }

    #[test]
    fn pedal_switch_mid_stream_stays_finite() {
        let mut m = prepared(CHORUS);
        set_by(&mut m, "feedback", 0.85);
        let x = sine(SR, 220.0, SR as usize / 4);
        let _ = process_stereo_in_blocks(&mut m, &x, 64);
        for mode in [
            FLANGER, PHASER, TREMOLO, VIBRATO, HARMONIC, ROTARY, UNIVIBE, CHORUS,
        ] {
            m.select_pedal(mode);
            for (i, p) in FAMILY.pedals[mode].params.iter().enumerate() {
                m.set_param(i, p.default_norm()); // control-side re-send
            }
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
            assert_finite("after pedal switch L", &l);
            assert_finite("after pedal switch R", &r);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            let x = silence(8_192);
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 512);
            assert!(rms(&l) == 0.0 && rms(&r) == 0.0, "{name} must stay silent");
        }
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            for (mode, _) in pedals() {
                let mut m = Modulation::new();
                m.prepare(sr);
                m.select_pedal(mode);
                for (i, p) in FAMILY.pedals[mode].params.iter().enumerate() {
                    m.set_param(i, p.default_norm());
                }
                for chunk in [32usize, 483, 1_024] {
                    let x = sine(sr, 440.0, 4_096);
                    let (l, r) = process_stereo_in_blocks(&mut m, &x, chunk);
                    assert_finite("mod multirate L", &l);
                    assert_finite("mod multirate R", &r);
                }
            }
        }
    }
}
