//! Delay: a family of three echo pedals behind one chain slot (PRD 001/004),
//! all built on one interpolated-read delay engine. Each pedal owns its
//! faceplate — its own knob set, captions, and voicing:
//!
//! - **digital**: pristine, full-bandwidth repeats, no feedback saturation,
//!   the longest time range, no modulation (Time/Feedback/Mix/Tone).
//! - **tape**: warm — soft-saturated feedback that darkens each pass, and a
//!   two-LFO wobble (slow **Wow** + fast **Flutter**) for that hair of chorus
//!   (Time/Feedback/Mix/Tone/Wow/Flutter).
//! - **vintage**: bucket-brigade voiced — dark and narrow, harder feedback
//!   compression, one chorus-y **Mod** LFO, the shortest time range
//!   (Time/Feedback/Mix/Tone/Mod).
//!
//! One engine, three voicings: the per-sample loop `match`es the voice's
//! [`VoiceDef`] constants (like [`crate::modulation`]'s `match mode`) — no
//! per-sample vtable. Switching pedals keeps the buffer (the tail rings on)
//! and the incoming pedal's shadow values are re-sent by the control side
//! (PRD 001). The `tone` knob sweeps a one-pole in the feedback path (dark ⇄
//! bright, compounding per repeat); its coefficient is rebuilt only when the
//! knob actually moves. `tape`/`vintage` soft-clip the feedback write
//! (unity small-signal, bounded loud — repeats compress instead of running
//! away); `digital` stays linear.
//!
//! **Tap tempo** (PRD 004) is a control-side feature: the `subdivision`
//! stepped param is stored here and shown in the UI, but the tap button
//! itself lives in the GUI and only ever sets the `time` param. The engine
//! treats `subdivision` as a no-op — it is a modifier for the control-side
//! tap→time math, not an audio parameter (there is no host-tempo sync in v1).

mod digital;
mod tape;
mod vintage;

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::blocks::onepole_hz;
use crate::blocks::smooth::Smoothed;

/// Longest delay any voice offers (digital), plus room for the read head to
/// wander under modulation without wrapping past the write head.
const MAX_DELAY_MS: f32 = 2_000.0;
const MOD_HEADROOM_MS: f32 = 60.0;

/// Tap-tempo note values and their ratio to the tapped beat (a quarter note).
/// Shared by every voice's faceplate; the GUI reads the ratio to turn a
/// tapped tempo into a delay time (PRD 004).
pub const SUBDIVISIONS: &[&str] = &["1/4", "dotted 1/8", "1/8 triplet", "1/8", "1/16"];
const SUBDIVISION_RATIOS: [f32; 5] = [1.0, 0.75, 1.0 / 3.0, 0.5, 0.25];

/// The ratio (of the tapped quarter note) for a subdivision index; `1.0` for
/// anything out of range. The GUI's tap button uses this to derive the time.
pub fn subdivision_ratio(index: usize) -> f32 {
    SUBDIVISION_RATIOS.get(index).copied().unwrap_or(1.0)
}

// --- shared faceplate parameters ---
// Every voice reuses these keys/ranges so the shared knobs mean the same
// thing across pedals (and old flat `delay` params migrate cleanly); each
// voice picks its own defaults and time/feedback ceilings.

const fn time_param(max_ms: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "time",
        name: "Time",
        unit: "ms",
        range: Range::Log {
            min: 20.0,
            max: max_ms,
        },
        default,
        smoothing_ms: 150.0,
    }
}

const fn feedback_param(max: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "feedback",
        name: "Feedback",
        unit: "",
        range: Range::Linear { min: 0.0, max },
        default,
        smoothing_ms: 20.0,
    }
}

const fn mix_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 20.0,
    }
}

/// Repeat brightness `0..1` (dark ⇄ bright); mapped to a feedback-path
/// lowpass corner over each voice's own `[tone_min_hz, tone_max_hz]`.
const fn tone_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "tone",
        name: "Tone",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 30.0,
    }
}

/// A modulation-depth knob `0..1` (Wow/Flutter/Mod); the LFO rate and the
/// max pitch deviation are the voice's fixed character.
const fn depth_param(key: &'static str, name: &'static str, default: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 50.0,
    }
}

/// Tap-tempo note value — a control-side modifier (see the module docs), so
/// it is not smoothed and does nothing in the audio path.
const SUBDIVISION: ParamDesc = ParamDesc {
    key: "subdivision",
    name: "Div",
    unit: "",
    range: Range::Stepped {
        labels: SUBDIVISIONS,
    },
    default: 0.0,
    smoothing_ms: 0.0,
};

/// The delay family, in menu order. Pinned to `lh_core::preset::DELAY_PEDALS`
/// (the v3→v4 migration) by a test below.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "delay",
    name: "Delay",
    pedals: &[&digital::DESC, &tape::DESC, &vintage::DESC],
};

pub const VOICE_COUNT: usize = 3;

/// Which engine control a voice's param position drives. `Subdivision` is a
/// control-side modifier (no audio effect); everything else lands on a
/// smoother.
#[derive(Clone, Copy)]
enum Ctl {
    Time,
    Feedback,
    Mix,
    Tone,
    /// Slow LFO depth — tape Wow, vintage Mod.
    ModA,
    /// Fast LFO depth — tape Flutter.
    ModB,
    Subdivision,
}

/// One voice's faceplate, param→control routing (same length as the
/// faceplate), and voicing constants. The engine reads these in the hot loop
/// instead of dispatching through a trait.
pub struct VoiceDef {
    pub desc: &'static EffectDesc,
    controls: &'static [Ctl],
    /// Soft-clip the feedback write (tape/vintage) vs. stay linear (digital).
    saturate: bool,
    /// Pre-gain into `tanh`; unity small-signal, `1/drive` ceiling — bigger =
    /// more compression of loud/built-up repeats.
    drive: f32,
    /// Tone-knob endpoints: the feedback lowpass corner at knob 0 and 1.
    tone_min_hz: f32,
    tone_max_hz: f32,
    /// Slow LFO (Wow / Mod): rate and max read-head deviation at depth 1.
    lfo_a_hz: f32,
    mod_a_ms: f32,
    /// Fast LFO (Flutter): rate and deviation; `0` disables it.
    lfo_b_hz: f32,
    mod_b_ms: f32,
}

/// The delay voice registry, aligned with [`FAMILY`]`.pedals`.
pub static VOICES: [VoiceDef; VOICE_COUNT] = [digital::VOICE, tape::VOICE, vintage::VOICE];

/// Unity small-signal, bounded loud: `tanh(drive·x)/drive`. The `1/drive`
/// ceiling keeps a saturated feedback loop finite forever (RT rule 7).
#[inline]
fn soft_clip(x: f32, drive: f32) -> f32 {
    (x * drive).tanh() / drive
}

/// The feedback lowpass corner for a tone-knob position, geometric over the
/// voice's range.
#[inline]
fn tone_cutoff(def: &VoiceDef, tone: f32) -> f32 {
    def.tone_min_hz * (def.tone_max_hz / def.tone_min_hz).powf(tone.clamp(0.0, 1.0))
}

/// One channel's circular buffer and feedback tone-filter memory.
struct DelayChannel {
    buf: Vec<f32>,
    write: usize,
    tone_lp: f32,
}

impl DelayChannel {
    /// One sample: interpolated read `delay_smp + offset` behind the write
    /// head, tone-filter it, write `x + feedback·wet` (soft-clipped for
    /// saturating voices), return the filtered wet tap.
    #[inline]
    fn step(
        &mut self,
        x: f32,
        delay_smp: f32,
        offset: f32,
        feedback: f32,
        tone_coeff: f32,
        def: &VoiceDef,
    ) -> f32 {
        let len = self.buf.len();
        // Fold the modulation into the read distance and clamp once, so the
        // read head can never wrap past the write head.
        let d = (delay_smp + offset).clamp(1.0, (len - 2) as f32);
        let rp = self.write as f32 - d + len as f32;
        let i0 = rp as usize;
        let frac = rp - i0 as f32;
        let s0 = self.buf[i0 % len];
        let s1 = self.buf[(i0 + 1) % len];
        let raw = s0 + frac * (s1 - s0);

        // Tone lowpass colors the wet and, sitting in the loop, darkens each
        // successive repeat (analog-style).
        self.tone_lp += tone_coeff * (raw - self.tone_lp);
        if self.tone_lp.abs() < 1e-20 {
            self.tone_lp = 0.0;
        }
        let wet = self.tone_lp;

        let mut fb_in = x + feedback * wet;
        if def.saturate {
            fb_in = soft_clip(fb_in, def.drive);
        }
        if fb_in.abs() < 1e-15 {
            fb_in = 0.0;
        }
        self.buf[self.write] = fb_in;
        self.write = (self.write + 1) % len;
        wet
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.tone_lp = 0.0;
    }
}

pub struct Delay {
    sample_rate: u32,
    voice: usize,
    ch: [DelayChannel; 2],
    time_ms: f32,
    delay_smp: Smoothed,
    feedback: Smoothed,
    mix: Smoothed,
    tone: Smoothed,
    mod_a: Smoothed,
    mod_b: Smoothed,
    /// Shared LFO phases (the right channel reads them a quarter-cycle ahead
    /// for a little width).
    phase_a: f32,
    phase_b: f32,
    /// Cached tone lowpass coefficient, rebuilt only when `tone` moves.
    tone_last: f32,
    tone_coeff: f32,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    pub fn new() -> Self {
        let channel = || DelayChannel {
            buf: Vec::new(),
            write: 0,
            tone_lp: 0.0,
        };
        // Defaults mirror the digital faceplate (voice 0); a pedal switch
        // re-sends the incoming voice's values from the control shadow.
        Self {
            sample_rate: 48_000,
            voice: 0,
            ch: [channel(), channel()],
            time_ms: 350.0,
            delay_smp: Smoothed::new(0.0),
            feedback: Smoothed::new(0.35),
            mix: Smoothed::new(0.25),
            tone: Smoothed::new(0.7),
            mod_a: Smoothed::new(0.0),
            mod_b: Smoothed::new(0.0),
            phase_a: 0.0,
            phase_b: 0.0,
            tone_last: f32::NAN, // force a coefficient build on the first block
            tone_coeff: 0.3,
        }
    }
}

impl Effect for Delay {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.voice
    }

    /// A delay rings: long time × high feedback can hold audible repeats
    /// for several seconds (tape/vintage self-oscillate — bounded, ended by
    /// the spill lane's silence/forced-decay logic, PRD 010).
    fn tail_seconds(&self) -> f32 {
        8.0
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal != self.voice && pedal < VOICE_COUNT {
            self.voice = pedal;
            // Keep the buffers so the tail rings through the switch; drop the
            // filter memory and force a tone rebuild so the incoming voice's
            // coloring takes hold cleanly. Unused LFOs are gated to zero by
            // the voice's `mod_*_ms` constants, so stale depths never leak.
            for ch in &mut self.ch {
                ch.tone_lp = 0.0;
            }
            self.tone_last = f32::NAN;
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        let cap = ((MAX_DELAY_MS + MOD_HEADROOM_MS) * 1e-3 * sample_rate as f32) as usize + 4;
        for ch in &mut self.ch {
            ch.buf = vec![0.0; cap];
        }
        self.delay_smp.configure(150.0, sample_rate);
        self.feedback.configure(20.0, sample_rate);
        self.mix.configure(20.0, sample_rate);
        self.tone.configure(30.0, sample_rate);
        self.mod_a.configure(50.0, sample_rate);
        self.mod_b.configure(50.0, sample_rate);
        self.delay_smp
            .set_target(self.time_ms * 1e-3 * sample_rate as f32);
        self.delay_smp.snap_to_target();
        self.feedback.snap_to_target();
        self.mix.snap_to_target();
        self.tone.snap_to_target();
        self.mod_a.snap_to_target();
        self.mod_b.snap_to_target();
        self.tone_last = f32::NAN;
        self.reset();
    }

    fn reset(&mut self) {
        for ch in &mut self.ch {
            ch.clear();
        }
        self.phase_a = 0.0;
        self.phase_b = 0.0;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let def = &VOICES[self.voice];
        let (Some(ctl), Some(param)) = (def.controls.get(index), def.desc.params.get(index)) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Time => {
                self.time_ms = real;
                self.delay_smp
                    .set_target(real * 1e-3 * self.sample_rate as f32);
            }
            Ctl::Feedback => self.feedback.set_target(real),
            Ctl::Mix => self.mix.set_target(real),
            Ctl::Tone => self.tone.set_target(real),
            Ctl::ModA => self.mod_a.set_target(real),
            Ctl::ModB => self.mod_b.set_target(real),
            Ctl::Subdivision => {} // control-side only (see module docs)
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = self.ch[0].buf.len();
        if len == 0 {
            return; // prepare() not called yet
        }
        let def = &VOICES[self.voice];
        let sr = self.sample_rate as f32;
        let ms_to_smp = sr * 1e-3;
        let a_inc = std::f32::consts::TAU * def.lfo_a_hz / sr;
        let b_inc = std::f32::consts::TAU * def.lfo_b_hz / sr;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let d = self.delay_smp.tick();
            let fb = self.feedback.tick();
            let mix = self.mix.tick();
            let tone = self.tone.tick();
            let mod_a = self.mod_a.tick();
            let mod_b = self.mod_b.tick();
            // Rebuild the tone coefficient only when the knob is actually
            // moving (the exp/powf stay out of the settled hot path).
            if tone != self.tone_last {
                self.tone_last = tone;
                self.tone_coeff = onepole_hz(tone_cutoff(def, tone), sr);
            }

            let la = self.phase_a.sin();
            let lb = self.phase_b.sin();
            let la_r = (self.phase_a + std::f32::consts::FRAC_PI_2).sin();
            let lb_r = (self.phase_b + std::f32::consts::FRAC_PI_2).sin();
            self.phase_a += a_inc;
            if self.phase_a >= std::f32::consts::TAU {
                self.phase_a -= std::f32::consts::TAU;
            }
            self.phase_b += b_inc;
            if self.phase_b >= std::f32::consts::TAU {
                self.phase_b -= std::f32::consts::TAU;
            }
            let dev_a = mod_a * def.mod_a_ms * ms_to_smp;
            let dev_b = mod_b * def.mod_b_ms * ms_to_smp;
            let off_l = dev_a * la + dev_b * lb;
            let off_r = dev_a * la_r + dev_b * lb_r;

            let wet_l = self.ch[0].step(*l, d, off_l, fb, self.tone_coeff, def);
            let wet_r = self.ch[1].step(*r, d, off_r, fb, self.tone_coeff, def);
            *l += wet_l * mix;
            *r += wet_r * mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, impulse, peak, process_in_blocks, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared(voice: usize) -> Delay {
        let mut d = Delay::new();
        d.prepare(SR);
        d.select_pedal(voice);
        d
    }

    /// Set a param by real value at the active voice's `index`.
    fn set(d: &mut Delay, index: usize, real: f32) {
        let param = &VOICES[d.voice].desc.params[index];
        d.set_param(index, param.range.to_norm(real));
    }

    /// The param index of `key` on the active voice.
    fn idx(d: &Delay, key: &str) -> usize {
        VOICES[d.voice].desc.param_index(key).unwrap()
    }

    fn set_by(d: &mut Delay, key: &str, real: f32) {
        let i = idx(d, key);
        set(d, i, real);
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.pedals.len(), VOICES.len());
        for (def, desc) in VOICES.iter().zip(FAMILY.pedals) {
            assert!(std::ptr::eq(def.desc, *desc), "VOICES aligned with FAMILY");
            assert_eq!(def.controls.len(), def.desc.params.len());
        }
        // Keys are unique (REPL/preset-facing identifiers).
        for (i, a) in FAMILY.pedals.iter().enumerate() {
            for b in &FAMILY.pedals[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
        // The v3→v4 migration references voices by key and index; pin them.
        let keys: Vec<&str> = FAMILY.pedals.iter().map(|p| p.key).collect();
        assert_eq!(keys, lh_core::preset::DELAY_PEDALS);
        // Each voice wears its own faceplate.
        let captions =
            |i: usize| -> Vec<&str> { FAMILY.pedals[i].params.iter().map(|p| p.name).collect() };
        assert_eq!(captions(0), ["Time", "Feedback", "Mix", "Tone", "Div"]);
        assert_eq!(
            captions(1),
            ["Time", "Feedback", "Mix", "Tone", "Wow", "Flutter", "Div"]
        );
        assert_eq!(
            captions(2),
            ["Time", "Feedback", "Mix", "Tone", "Mod", "Div"]
        );
        // Every voice shares the tap subdivision selector.
        for pedal in FAMILY.pedals {
            assert!(matches!(
                pedal.params[pedal.param_index("subdivision").unwrap()].range,
                Range::Stepped { .. }
            ));
        }
        assert_eq!(subdivision_ratio(0), 1.0);
        assert!((subdivision_ratio(1) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn echoes_arrive_at_the_configured_time() {
        // A digital echo lands where `time` says and decays via feedback.
        let mut d = prepared(0);
        set_by(&mut d, "time", 100.0); // 100 ms = 4800 samples
        set_by(&mut d, "feedback", 0.5);
        set_by(&mut d, "mix", 1.0);
        // Let the time smoother settle before measuring.
        let mut warm = silence(SR as usize);
        let mut warm_r = silence(SR as usize);
        d.process(&mut warm, &mut warm_r);

        let x = impulse(SR as usize, 0);
        let y = process_in_blocks(&mut d, &x, 512);
        assert_finite("delay", &y);
        let p1 = peak(&y[4_700..4_900]);
        let p2 = peak(&y[9_500..9_700]);
        assert!(p1 > 0.4, "first echo missing, peak {p1}");
        assert!(p2 > 0.1 && p2 < p1, "second echo must decay: {p1} → {p2}");
        assert!(peak(&y[10..4_600]) < 1e-3, "nothing before the first echo");
    }

    #[test]
    fn every_voice_is_finite_bounded_and_silent_in_silent_out() {
        for (voice, def) in VOICES.iter().enumerate() {
            // Max feedback held under a sustained tone: the honest runaway
            // test. Saturating voices compress; digital decays.
            let mut d = prepared(voice);
            let fb_max = def.desc.params[idx(&d, "feedback")].range.max();
            set_by(&mut d, "feedback", fb_max);
            set_by(&mut d, "mix", 1.0);
            let x = sine(SR, 220.0, SR as usize);
            let y = process_in_blocks(&mut d, &x, 256);
            assert_finite(def.desc.key, &y);
            assert!(
                peak(&y) < 12.0,
                "{}: feedback must stay bounded, peak {}",
                def.desc.key,
                peak(&y)
            );

            d.reset();
            let s = silence(SR as usize / 2);
            let y = process_in_blocks(&mut d, &s, 128);
            assert!(rms(&y) == 0.0, "{}: silence in → silence out", def.desc.key);
        }
    }

    #[test]
    fn self_oscillating_voices_keep_ringing() {
        // Analog signature: at max feedback the saturating voices (tape 1.0,
        // vintage 1.05) sit at/above unity loop gain and self-sustain — the
        // soft clip keeps that bounded — while the linear digital delay
        // (max 0.9) decays away. A short burst charges the loop; the tail is
        // measured after four seconds of silence.
        let tail_rms = |voice: usize| {
            let mut d = prepared(voice);
            set_by(&mut d, "time", 120.0);
            set_by(&mut d, "tone", 0.6); // pass the 200 Hz charge freely
            let fb_max = VOICES[voice].desc.params[idx(&d, "feedback")].range.max();
            set_by(&mut d, "feedback", fb_max);
            set_by(&mut d, "mix", 1.0);
            let mut x = sine(SR, 200.0, SR as usize / 5); // 200 ms burst
            for s in &mut x {
                *s *= 0.8;
            }
            x.extend(silence(SR as usize * 4)); // 4 s of silence to ring out
            let y = process_in_blocks(&mut d, &x, 256);
            f64::from(rms(&y[y.len() - SR as usize / 2..])) // last 0.5 s
        };
        let digital = tail_rms(0);
        let tape = tail_rms(1);
        let vintage = tail_rms(2);
        assert!(
            tape > 2.5 * digital.max(1e-4) && vintage > 2.5 * digital.max(1e-4),
            "saturating voices must self-sustain: digital {digital:.5}, tape {tape:.5}, vintage {vintage:.5}"
        );
    }

    /// RMS of the wet tail only (output − dry) for a `freq` tone at `voice`
    /// with the given tone knob — isolates the echo from the always-full dry.
    fn wet_hf(voice: usize, freq: f32, tone: f32) -> f64 {
        let mut d = prepared(voice);
        set_by(&mut d, "time", 60.0);
        set_by(&mut d, "feedback", 0.6);
        set_by(&mut d, "mix", 1.0);
        set_by(&mut d, "tone", tone);
        let mut warm = silence(SR as usize / 2);
        let mut warm_r = silence(SR as usize / 2);
        d.process(&mut warm, &mut warm_r);
        let x = sine(SR, freq, SR as usize);
        let y = process_in_blocks(&mut d, &x, 256);
        let wet: Vec<f32> = y[SR as usize / 2..]
            .iter()
            .zip(&x[SR as usize / 2..])
            .map(|(o, i)| o - i)
            .collect();
        f64::from(rms(&wet))
    }

    #[test]
    fn tone_knob_sets_repeat_brightness() {
        // A bright tone keeps more high end in the echo than a dark one.
        for voice in 0..VOICE_COUNT {
            let dark = wet_hf(voice, 4_000.0, 0.0);
            let bright = wet_hf(voice, 4_000.0, 1.0);
            assert!(
                bright > 1.5 * dark,
                "voice {voice}: bright echo must keep more 4 kHz than dark ({bright:.4} vs {dark:.4})"
            );
        }
    }

    #[test]
    fn modulated_voices_wobble_and_digital_does_not() {
        // digital is LTI: fed a settled 250 Hz tone (an integer number of
        // periods per block), two consecutive blocks match to filter noise.
        // tape/vintage sweep the read head, so their consecutive blocks
        // diverge by a large margin.
        let block_diff = |voice: usize, depth_key: &str| -> f32 {
            let mut d = prepared(voice);
            set_by(&mut d, "time", 40.0);
            set_by(&mut d, "feedback", 0.25); // low Q → digital settles fast
            set_by(&mut d, "mix", 1.0);
            if let Some(i) = VOICES[voice].desc.param_index(depth_key) {
                set(&mut d, i, 1.0);
            }
            // Warm to steady state (200 Hz = 240 samples, phase-continuous).
            let warm = sine(SR, 200.0, SR as usize * 2);
            let _ = process_in_blocks(&mut d, &warm, 512);
            let x = sine(SR, 200.0, 4_800); // exactly 20 periods
            let a = process_in_blocks(&mut d, &x, 4_800);
            let b = process_in_blocks(&mut d, &x, 4_800);
            a.iter()
                .zip(&b)
                .map(|(p, q)| (p - q).abs())
                .fold(0.0, f32::max)
        };
        let (d0, d1, d2) = (
            block_diff(0, "tone"),
            block_diff(1, "wow"),
            block_diff(2, "mod"),
        );
        assert!(d0 < 1e-3, "digital steady state must repeat (diff {d0})");
        assert!(d1 > 1e-2, "tape wow must modulate (diff {d1})");
        assert!(d2 > 1e-2, "vintage mod must modulate (diff {d2})");
    }

    #[test]
    fn vintage_is_darker_than_digital_at_the_same_tone() {
        // Same knobs, BBD voicing: the vintage feedback path rolls off far
        // more high end than the pristine digital one (wet only).
        let digital = wet_hf(0, 5_000.0, 0.5);
        let vintage = wet_hf(2, 5_000.0, 0.5);
        assert!(
            digital > 2.0 * vintage,
            "vintage must be darker than digital ({digital:.4} vs {vintage:.4})"
        );
    }

    #[test]
    fn time_changes_do_not_produce_nan_or_clicks() {
        for voice in 0..VOICE_COUNT {
            let mut d = prepared(voice);
            set_by(&mut d, "mix", 1.0);
            let mut x = sine(SR, 330.0, SR as usize);
            let mut xr = x.clone();
            let (a, b) = x.split_at_mut(SR as usize / 2);
            let (ar, br) = xr.split_at_mut(SR as usize / 2);
            d.process(a, ar);
            set_by(&mut d, "time", 20.0); // slam short mid-flight
            d.process(b, br);
            assert_finite("delay sweep", &x);
        }
    }

    #[test]
    fn pedal_switch_mid_note_stays_finite() {
        let mut d = prepared(0);
        set_by(&mut d, "feedback", 0.6);
        set_by(&mut d, "mix", 0.6);
        let x = sine(SR, 220.0, SR as usize / 2);
        let mut left = x.clone();
        let mut right = x.clone();
        for (i, (bl, br)) in left.chunks_mut(64).zip(right.chunks_mut(64)).enumerate() {
            d.select_pedal(i % VOICE_COUNT);
            d.process(bl, br);
        }
        assert_finite("delay pedal switch", &left);
        assert!(peak(&left) < 8.0);
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            for voice in 0..VOICE_COUNT {
                let mut d = Delay::new();
                d.prepare(sr);
                d.select_pedal(voice);
                set_by(&mut d, "feedback", 0.7);
                set_by(&mut d, "mix", 0.5);
                for chunk in [32usize, 483, 1_024] {
                    let x = sine(sr, 440.0, 4_096);
                    let y = process_in_blocks(&mut d, &x, chunk);
                    assert_finite("delay multirate", &y);
                }
            }
        }
    }
}
