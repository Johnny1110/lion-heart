//! The modulation family: one slot, four pedals — chorus, flanger, phaser,
//! tremolo — each with its own faceplate (PRD 001):
//!
//! - **chorus**: delay line swept 2–14 ms, gentle feedback
//!   (rate/depth/feedback/mix)
//! - **flanger**: delay line swept 1–5 ms, prominent feedback
//!   (rate/depth/feedback/mix)
//! - **phaser**: four first-order allpass stages, cutoff swept 230–2100 Hz
//!   (rate/depth/feedback/mix)
//! - **tremolo**: amplitude LFO (rate/depth) — wet-only by construction;
//!   the v2 `mix` knob was redundant with depth (`depth' = depth × mix`,
//!   folded by the preset migration).
//!
//! All four share one LFO and one pair of voices; switching pedals resets
//! the voice state (a brief discontinuity while auditioning, never NaN);
//! continuous params morph smoothly.
//!
//! Stereo (M7): two independent voices share the LFO, with the right
//! channel's phase offset a quarter cycle for chorus/flanger/phaser (width)
//! and half a cycle for tremolo (auto-pan).

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::smooth::Smoothed;

const CHORUS: usize = 0;
const FLANGER: usize = 1;
const PHASER: usize = 2;
const TREMOLO: usize = 3;

/// Longest modulated delay (chorus max) plus headroom.
const MAX_DELAY_MS: f32 = 20.0;
const PHASER_STAGES: usize = 4;

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

// chorus/flanger/phaser keep the v2 keys, ranges, and defaults, so sparse
// v2 presets migrate without pinning; the tremolo faceplate is its own.
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

static TREMOLO_PARAMS: [ParamDesc; 2] = [rate(5.0), depth(0.5)];
static TREMOLO_DESC: EffectDesc = EffectDesc {
    key: "tremolo",
    name: "Tremolo",
    params: &TREMOLO_PARAMS,
};

/// The modulation family, in menu order. Pinned to
/// `lh_core::preset::MOD_PEDALS` (the v2 migration) by a test below.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "mod",
    name: "Modulation",
    pedals: &[&CHORUS_DESC, &FLANGER_DESC, &PHASER_DESC, &TREMOLO_DESC],
};

/// One channel's voice: delay line (chorus/flanger), allpass chain (phaser),
/// and feedback memory. Two of these make the stereo pair.
struct Voice {
    buf: Vec<f32>,
    write: usize,
    ap_x1: [f32; PHASER_STAGES],
    ap_y1: [f32; PHASER_STAGES],
    fb: f32,
}

impl Voice {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            write: 0,
            ap_x1: [0.0; PHASER_STAGES],
            ap_y1: [0.0; PHASER_STAGES],
            fb: 0.0,
        }
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.ap_x1 = [0.0; PHASER_STAGES];
        self.ap_y1 = [0.0; PHASER_STAGES];
        self.fb = 0.0;
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

    /// One wet sample of the selected mode driven by this channel's LFO value.
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
    ) -> f32 {
        match mode {
            CHORUS | FLANGER => {
                let delay_ms = if mode == CHORUS {
                    8.0 + 6.0 * depth * lfo // 2..14 ms
                } else {
                    1.0 + 4.0 * depth * (0.5 + 0.5 * lfo) // 1..5 ms
                };
                let delay_smp = (delay_ms * ms).clamp(1.0, (self.buf.len() - 2) as f32);
                let tap = self.read_delayed(delay_smp);
                self.buf[self.write] = x + feedback * tap;
                self.write = (self.write + 1) % self.buf.len();
                tap
            }
            PHASER => {
                // Sweep the allpass corner geometrically around 700 Hz.
                let fc = 700.0 * 3f32.powf(lfo * depth);
                let t = (std::f32::consts::PI * fc / sample_rate).tan();
                let a = (1.0 - t) / (1.0 + t);
                let mut y = x + feedback * self.fb;
                for stage in 0..PHASER_STAGES {
                    let out = -a * y + self.ap_x1[stage] + a * self.ap_y1[stage];
                    self.ap_x1[stage] = y;
                    self.ap_y1[stage] = out;
                    y = out;
                }
                self.fb = y;
                y
            }
            TREMOLO => x * (1.0 - depth * (0.5 + 0.5 * lfo)),
            _ => x, // unreachable: select_pedal rejects out-of-range
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
    phase: f32,
    voices: [Voice; 2],
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
            phase: 0.0,
            voices: [Voice::new(), Voice::new()],
        }
    }

    fn clear_voices(&mut self) {
        for voice in &mut self.voices {
            voice.clear();
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
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        for voice in &mut self.voices {
            voice.buf = vec![0.0; (MAX_DELAY_MS * 1e-3 * self.sample_rate) as usize + 4];
        }
        // Smoothing times mirror the faceplate descs.
        for (smoothed, ms) in [
            (&mut self.rate, 80.0),
            (&mut self.depth, 50.0),
            (&mut self.feedback, 50.0),
            (&mut self.mix, 30.0),
        ] {
            smoothed.configure(ms, sample_rate);
            smoothed.snap_to_target();
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.clear_voices();
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let Some(param) = FAMILY.pedals[self.mode].params.get(index) else {
            return;
        };
        let real = param.range.to_real(normalized);
        // Rate and depth lead every faceplate; feedback/mix exist only on
        // the delay/allpass pedals (the desc lookup above already gates
        // tremolo's two-knob face).
        match index {
            0 => self.rate.set_target(real),
            1 => self.depth.set_target(real),
            2 => self.feedback.set_target(real),
            3 => self.mix.set_target(real),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.voices[0].buf.is_empty() {
            return; // prepare() not called yet
        }
        let ms = self.sample_rate * 1e-3;
        // Right-channel LFO offset: quadrature for width, opposite phase for
        // tremolo (auto-pan).
        let offset = if self.mode == TREMOLO {
            std::f32::consts::PI
        } else {
            std::f32::consts::FRAC_PI_2
        };
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let rate = self.rate.tick();
            let depth = self.depth.tick();
            let feedback = self.feedback.tick();
            let mix = self.mix.tick();

            self.phase += std::f32::consts::TAU * rate / self.sample_rate;
            if self.phase >= std::f32::consts::TAU {
                self.phase -= std::f32::consts::TAU;
            }
            let lfo_l = self.phase.sin();
            let lfo_r = (self.phase + offset).sin();

            let (dry_l, dry_r) = (*l, *r);
            let wet_l = self.voices[0].step(
                self.mode,
                dry_l,
                lfo_l,
                depth,
                feedback,
                ms,
                self.sample_rate,
            );
            let wet_r = self.voices[1].step(
                self.mode,
                dry_r,
                lfo_r,
                depth,
                feedback,
                ms,
                self.sample_rate,
            );
            if self.mode == TREMOLO {
                // Wet-only: depth alone sets the effect strength.
                *l = wet_l;
                *r = wet_r;
            } else {
                *l = dry_l + mix * (wet_l - dry_l);
                *r = dry_r + mix * (wet_r - dry_r);
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
        m
    }

    /// Set a param by real value at the active pedal's `index`.
    fn set(m: &mut Modulation, index: usize, real: f32) {
        let param = &FAMILY.pedals[m.pedal_index()].params[index];
        m.set_param(index, param.range.to_norm(real));
    }

    /// `(index, name)` pedal iterator for the character loops.
    fn pedals() -> impl Iterator<Item = (usize, &'static str)> {
        FAMILY.pedals.iter().enumerate().map(|(i, p)| (i, p.key))
    }

    #[test]
    fn registry_is_consistent() {
        let keys: Vec<&str> = FAMILY.pedals.iter().map(|p| p.key).collect();
        assert_eq!(keys, lh_core::preset::MOD_PEDALS);
        // Tremolo's faceplate is rate/depth only — no dead knobs.
        assert_eq!(TREMOLO_DESC.params.len(), 2);
        assert!(TREMOLO_DESC.param_index("mix").is_none());
        assert!(TREMOLO_DESC.param_index("feedback").is_none());
        // The others keep the four v2 knobs (keys, ranges, defaults).
        for pedal in [&CHORUS_DESC, &FLANGER_DESC, &PHASER_DESC] {
            assert_eq!(pedal.params.len(), 4);
            assert_eq!(pedal.param_index("mix"), Some(3));
        }
    }

    #[test]
    fn all_modes_render_finite_bounded_audio() {
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            if FAMILY.pedals[mode].params.len() > 2 {
                set(&mut m, 2, 0.85); // max feedback
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
    fn stereo_channels_decorrelate() {
        // The quadrature LFO offset must make L and R audibly different.
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            set(&mut m, 0, 2.0);
            set(&mut m, 1, 1.0);
            if FAMILY.pedals[mode].params.len() > 3 {
                set(&mut m, 3, 1.0);
            }
            let x = sine(SR, 220.0, SR as usize / 2);
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
            let diff: f32 = l
                .iter()
                .zip(&r)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(diff > 0.05, "{name} must be wide, max |L-R| = {diff}");
        }
    }

    #[test]
    fn zero_mix_or_depth_is_bit_exact_dry() {
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            // Chorus/flanger/phaser: mix 0 is dry. Tremolo has no mix; its
            // dry position is depth 0 (wet = x exactly).
            if FAMILY.pedals[mode].params.len() > 3 {
                set(&mut m, 3, 0.0);
            } else {
                set(&mut m, 1, 0.0);
            }
            // Let the smoothing decay all the way to the snap threshold
            // (~20 time constants) before comparing.
            let warm = sine(SR, 220.0, SR as usize);
            let _ = process_stereo_in_blocks(&mut m, &warm, 512);
            let x = sine(SR, 220.0, 8_192);
            let (l, r) = process_stereo_in_blocks(&mut m, &x, 512);
            assert_eq!(x, l, "{name} L must pass dry");
            assert_eq!(x, r, "{name} R must pass dry");
        }
    }

    #[test]
    fn output_is_time_varying() {
        // The same input block must not produce the same output twice in a
        // row — the LFO has moved. (Tremolo included: 4 Hz over 100 ms.)
        for (mode, name) in pedals() {
            let mut m = prepared(mode);
            set(&mut m, 0, 4.0);
            set(&mut m, 1, 1.0);
            if FAMILY.pedals[mode].params.len() > 3 {
                set(&mut m, 3, 1.0);
            }
            let x = sine(SR, 220.0, 4_800);
            let (first, _) = process_stereo_in_blocks(&mut m, &x, 4_800);
            let (second, _) = process_stereo_in_blocks(&mut m, &x, 4_800);
            assert_ne!(first, second, "{name} must modulate over time");
        }
    }

    #[test]
    fn tremolo_pumps_and_pans() {
        let mut m = prepared(TREMOLO);
        set(&mut m, 0, 4.0);
        set(&mut m, 1, 1.0);
        let x = sine(SR, 220.0, SR as usize); // 1 s = 4 LFO cycles
        let (l, r) = process_stereo_in_blocks(&mut m, &x, 64);
        // 25 ms windows: min RMS must dip far below max RMS on each channel,
        // and the channels must not dip together (opposite LFO phase).
        let win = SR as usize / 40;
        for (label, y) in [("L", &l), ("R", &r)] {
            let rms_per: Vec<f32> = y[SR as usize / 2..].chunks(win).map(rms).collect();
            let max = rms_per.iter().fold(0.0f32, |m, v| m.max(*v));
            let min = rms_per.iter().fold(f32::INFINITY, |m, v| m.min(*v));
            assert!(min < 0.4 * max, "tremolo {label} must pump: {min} vs {max}");
        }
        let sum_windows: Vec<f32> = l[SR as usize / 2..]
            .iter()
            .zip(&r[SR as usize / 2..])
            .map(|(a, b)| a + b)
            .collect::<Vec<_>>()
            .chunks(win)
            .map(rms)
            .collect();
        let max = sum_windows.iter().fold(0.0f32, |m, v| m.max(*v));
        let min = sum_windows.iter().fold(f32::INFINITY, |m, v| m.min(*v));
        assert!(
            min > 0.55 * max,
            "auto-pan: L+R must stay steadier than either side ({min} vs {max})"
        );
    }

    #[test]
    fn pedal_switch_mid_stream_stays_finite() {
        let mut m = prepared(CHORUS);
        set(&mut m, 2, 0.85);
        let x = sine(SR, 220.0, SR as usize / 4);
        let _ = process_stereo_in_blocks(&mut m, &x, 64);
        for mode in [FLANGER, PHASER, TREMOLO, CHORUS] {
            m.select_pedal(mode);
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
            for mode in 0..FAMILY.pedals.len() {
                let mut m = Modulation::new();
                m.prepare(sr);
                m.select_pedal(mode);
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
