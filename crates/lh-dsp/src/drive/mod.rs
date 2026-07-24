//! Drive: a family of overdrive pedals behind one chain slot. Each pedal
//! owns its faceplate (PRD 001): its own knob set, captions, and defaults —
//! TS9 has exactly three knobs, the evva five. Knobs are positions `0..=10`,
//! laid out like the face of the modelled pedal so nothing has to be
//! relearned.
//!
//! One pedal, one file: the circuit model, its faceplate, and its voicing
//! constants live together ([`ts9`], [`bd2`], …); this module owns the
//! family registry, the shared building blocks ([`OnePole`], [`Ramp`]), and
//! the [`Drive`] effect that hosts whichever circuit is selected.
//!
//! Every model runs its nonlinearity inside the shared 4× oversampler
//! ([`crate::blocks::oversample`]) and its linear tone stack at the base
//! rate. Knob smoothing is written once per chunk into trajectory buffers
//! shared by both channels.
//!
//! # Adding your own pedal
//!
//! 1. Create `src/drive/yourpedal.rs`: declare the faceplate (a `ParamDesc`
//!    table + an `EffectDesc`) and implement [`Circuit`] — the nonlinear
//!    `shape` pass at the oversampled rate, the linear `post` pass (tone
//!    stack, makeup) at the base rate, and `eq` if the face has per-band
//!    knobs.
//! 2. Register it: `mod yourpedal;` below, then **append** the desc to
//!    [`FAMILY`], a matching [`ModelDef`] to [`MODELS`], and its key to
//!    `lh_core::preset::DRIVE_PEDALS`. Append only — the v2 preset
//!    migration and plugin param ids reference pedals by position and key.
//!
//! Everything downstream picks the entry up from the registry: the GUI
//! pedal dropdown and knobs, REPL labels (`set drive.pedal ts9`), MIDI CC
//! mapping, preset save/load, and the plugin's per-pedal host params.

mod angry_charlie;
mod angry_charlie_v2;
mod bd2;
mod centaur;
mod classic;
mod evva;
mod fuzz_face;
mod jan_ray;
mod monster5150;
mod overdrive;
mod red_charlie;
mod screamer;
mod sd1;
mod ts9;

use lh_core::{FamilyDesc, ParamDesc, Range, db_to_lin, drive_law};

use crate::Effect;
use crate::blocks::oversample::{CHUNK, Oversampler4x};
use crate::blocks::smooth::Smoothed;

/// A pedal-style position knob `0..=10`.
const fn knob(key: &'static str, name: &'static str, default: f32, smoothing_ms: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 10.0,
        },
        default,
        smoothing_ms,
    }
}

/// The drive family, in menu order. Aligned with [`MODELS`] and pinned to
/// `lh_core::preset::DRIVE_PEDALS` (the v2 migration) by tests.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "drive",
    name: "Drive",
    pedals: &[
        &ts9::DESC,
        &bd2::DESC,
        &classic::DESC,
        &centaur::DESC,
        &evva::DESC,
        &red_charlie::DESC,
        &monster5150::DESC,
        &angry_charlie::DESC,
        &jan_ray::DESC,
        &fuzz_face::DESC,
        &overdrive::DESC,
        &screamer::DESC,
        &sd1::DESC,
        &angry_charlie_v2::DESC,
    ],
};

pub const MODEL_COUNT: usize = 14;

/// Which internal control a pedal's param position drives.
#[derive(Clone, Copy)]
enum Ctl {
    Drive,
    Tone,
    Level,
    Low,
    Mid,
    High,
}

/// One entry in the drive pedal registry: the faceplate, the param→control
/// routing (same length as the faceplate's params), and the circuit builder.
pub struct ModelDef {
    pub desc: &'static lh_core::EffectDesc,
    controls: &'static [Ctl],
    build: fn() -> Box<dyn Circuit>,
}

/// The drive pedal registry, aligned with [`FAMILY`]`.pedals`.
pub static MODELS: [ModelDef; MODEL_COUNT] = [
    ModelDef {
        desc: &ts9::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(ts9::Ts9::new()),
    },
    ModelDef {
        desc: &bd2::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(bd2::BluesDriver::new()),
    },
    ModelDef {
        desc: &classic::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(classic::Classic::new()),
    },
    ModelDef {
        desc: &centaur::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(centaur::Centaur::new()),
    },
    ModelDef {
        desc: &evva::DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::Mid, Ctl::High, Ctl::Level],
        build: || Box::new(evva::Evva::new()),
    },
    ModelDef {
        desc: &red_charlie::DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::Mid, Ctl::High, Ctl::Level],
        build: || Box::new(red_charlie::RedCharlie::new()),
    },
    ModelDef {
        desc: &monster5150::DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::Mid, Ctl::High, Ctl::Level],
        build: || Box::new(monster5150::Monster5150::new()),
    },
    ModelDef {
        desc: &angry_charlie::DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::Mid, Ctl::High, Ctl::Level],
        build: || Box::new(angry_charlie::AngryCharlie::new()),
    },
    ModelDef {
        desc: &jan_ray::DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::High, Ctl::Level],
        build: || Box::new(jan_ray::JanRay::new()),
    },
    ModelDef {
        desc: &fuzz_face::DESC,
        controls: &[Ctl::Drive, Ctl::Level],
        build: || Box::new(fuzz_face::FuzzFace::new()),
    },
    ModelDef {
        desc: &overdrive::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(overdrive::Overdrive::new()),
    },
    ModelDef {
        desc: &screamer::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(screamer::Screamer::new()),
    },
    ModelDef {
        desc: &sd1::DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(sd1::Sd1::new()),
    },
    ModelDef {
        desc: &angry_charlie_v2::DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::Mid, Ctl::High, Ctl::Level],
        build: || Box::new(angry_charlie_v2::AngryCharlieV2::new()),
    },
];

/// One channel of one drive model. Built off the audio thread
/// ([`ModelDef::build`]); both passes run on the audio thread and must obey
/// the real-time rules (no allocation, no locks, flush denormals).
pub trait Circuit: Send {
    fn prepare(&mut self, base_rate: f32, os_rate: f32);
    fn reset(&mut self);
    /// Nonlinear stage at the oversampled rate. `drive[i]` is the smoothed
    /// drive-knob position (0..10) for `block[i]`.
    fn shape(&mut self, block: &mut [f32], drive: &[f32]);
    /// Linear stage (tone stack, makeup) at the base rate; `tone[i]`
    /// likewise holds the smoothed tone-knob position.
    fn post(&mut self, block: &mut [f32], tone: &[f32]);
    /// 3-band EQ at the base rate; default no-op for models that don't have
    /// per-band controls. `low[i]` / `mid[i]` / `high[i]` are the smoothed
    /// knob positions (0..10) for `block[i]`.
    fn eq(&mut self, _block: &mut [f32], _low: &[f32], _mid: &[f32], _high: &[f32]) {}
}

// --- shared building blocks ---

use crate::blocks::onepole_hz as lp_coeff;

/// One-pole lowpass state; highpass is the input minus this. The flush keeps
/// decaying feedback out of denormal territory (RT rule 7).
#[derive(Default)]
struct OnePole {
    y: f32,
}

impl OnePole {
    #[inline]
    fn lp(&mut self, x: f32, c: f32) -> f32 {
        self.y += c * (x - self.y);
        if self.y.abs() < 1e-20 {
            self.y = 0.0;
        }
        self.y
    }

    fn reset(&mut self) {
        self.y = 0.0;
    }
}

/// Per-sample linear ramp between a chunk's first and last mapped knob
/// values — the mapping (`powf`, `exp`) runs twice per chunk instead of per
/// sample, while the pot still moves smoothly under it.
struct Ramp {
    v: f32,
    step: f32,
}

impl Ramp {
    fn over(traj: &[f32], f: impl Fn(f32) -> f32) -> Self {
        let a = f(traj[0]);
        let b = f(traj[traj.len() - 1]);
        Self {
            v: a,
            step: (b - a) / traj.len() as f32,
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let v = self.v;
        self.v += self.step;
        v
    }
}

/// Shared 3-band tone stack for pedals with Low/Mid/High faceplates:
/// one-pole shelves plus a cascaded-one-pole mid bandpass (LP at f·1.4 minus
/// a second LP at f/1.4). Knob 5 = flat (0 dB), 0/10 = full cut/boost —
/// low/high reach ±12 dB, mid ±10 dB. Each pedal keeps only its voicing
/// (the corner frequencies); the filter math and the dB knob laws live
/// here, once.
struct ToneStack {
    low_hz: f32,
    mid_hz: f32,
    high_hz: f32,
    lo: OnePole,
    mid_lp: OnePole,
    mid_hp: OnePole,
    hi: OnePole,
    c_lo: f32,
    c_mid_wide: f32,
    c_mid_narrow: f32,
    c_hi: f32,
}

impl ToneStack {
    fn new(low_hz: f32, mid_hz: f32, high_hz: f32) -> Self {
        Self {
            low_hz,
            mid_hz,
            high_hz,
            lo: OnePole::default(),
            mid_lp: OnePole::default(),
            mid_hp: OnePole::default(),
            hi: OnePole::default(),
            c_lo: 0.0,
            c_mid_wide: 0.0,
            c_mid_narrow: 0.0,
            c_hi: 0.0,
        }
    }

    fn prepare(&mut self, base_rate: f32) {
        self.c_lo = lp_coeff(self.low_hz, base_rate);
        self.c_mid_wide = lp_coeff(self.mid_hz * 1.4, base_rate);
        self.c_mid_narrow = lp_coeff(self.mid_hz / 1.4, base_rate);
        self.c_hi = lp_coeff(self.high_hz, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.lo.reset();
        self.mid_lp.reset();
        self.mid_hp.reset();
        self.hi.reset();
    }

    /// Apply the stack in place. Band gains are mapped from the smoothed
    /// knob trajectories with the shared [`Ramp`] — two `powf` per band per
    /// chunk instead of three per sample.
    fn process(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        let mut lo_gain = Ramp::over(low, |l| db_to_lin(-12.0 + 2.4 * l) - 1.0);
        let mut mid_gain = Ramp::over(mid, |m| db_to_lin(-10.0 + 2.0 * m) - 1.0);
        let mut hi_gain = Ramp::over(high, |h| db_to_lin(-12.0 + 2.4 * h) - 1.0);
        for s in block.iter_mut() {
            let x = *s;
            let lo = self.lo.lp(x, self.c_lo);
            let hi = x - self.hi.lp(x, self.c_hi);
            let bp_raw = self.mid_lp.lp(x, self.c_mid_wide);
            let bp = bp_raw - self.mid_hp.lp(bp_raw, self.c_mid_narrow);
            *s = x + lo_gain.tick() * lo + mid_gain.tick() * bp + hi_gain.tick() * hi;
        }
    }
}

// --- the effect ---

pub struct Drive {
    model: usize,
    os: [Oversampler4x; 2],
    /// One stereo pair of state per registered model, preallocated so a
    /// model switch on the audio thread is just an index change.
    circuits: Vec<[Box<dyn Circuit>; 2]>,
    /// Ticked at the oversampled rate (its trajectory feeds `shape`).
    drive_s: Smoothed,
    tone_s: Smoothed,
    level_s: Smoothed,
    low_s: Smoothed,
    mid_s: Smoothed,
    high_s: Smoothed,
    drive_traj: Vec<f32>,
    tone_traj: Vec<f32>,
    low_traj: Vec<f32>,
    mid_traj: Vec<f32>,
    high_traj: Vec<f32>,
}

impl Default for Drive {
    fn default() -> Self {
        Self::new()
    }
}

impl Drive {
    pub fn new() -> Self {
        Self {
            model: 0,
            os: [Oversampler4x::new(), Oversampler4x::new()],
            circuits: MODELS.iter().map(|m| [(m.build)(), (m.build)()]).collect(),
            // Control defaults match every pedal's faceplate defaults for
            // the controls it exposes (drive/gain 5, tone 5, level 6, EQ 5).
            drive_s: Smoothed::new(5.0),
            tone_s: Smoothed::new(5.0),
            level_s: Smoothed::new(6.0),
            low_s: Smoothed::new(5.0),
            mid_s: Smoothed::new(5.0),
            high_s: Smoothed::new(5.0),
            drive_traj: vec![0.0; 4 * CHUNK],
            tone_traj: vec![0.0; CHUNK],
            low_traj: vec![0.0; CHUNK],
            mid_traj: vec![0.0; CHUNK],
            high_traj: vec![0.0; CHUNK],
        }
    }
}

impl Effect for Drive {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.model
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal != self.model && pedal < self.circuits.len() {
            self.model = pedal;
            // Fresh filter state for the incoming pedal; the linear
            // oversampler keeps running. Like the modulation family,
            // switching mid-note is a brief discontinuity, never NaN.
            for circuit in &mut self.circuits[pedal] {
                circuit.reset();
            }
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        let base = sample_rate as f32;
        // Smoothing times mirror the knob descs; the drive trajectory is
        // ticked at the 4× oversampled rate the shaper runs at.
        self.drive_s.configure(20.0, sample_rate * 4);
        self.tone_s.configure(30.0, sample_rate);
        self.level_s.configure(20.0, sample_rate);
        self.low_s.configure(30.0, sample_rate);
        self.mid_s.configure(30.0, sample_rate);
        self.high_s.configure(30.0, sample_rate);
        self.drive_s.snap_to_target();
        self.tone_s.snap_to_target();
        self.level_s.snap_to_target();
        self.low_s.snap_to_target();
        self.mid_s.snap_to_target();
        self.high_s.snap_to_target();
        for pair in &mut self.circuits {
            for circuit in pair {
                circuit.prepare(base, base * 4.0);
            }
        }
        self.reset();
    }

    fn reset(&mut self) {
        for os in &mut self.os {
            os.reset();
        }
        for pair in &mut self.circuits {
            for circuit in pair {
                circuit.reset();
            }
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let def = &MODELS[self.model];
        let (Some(ctl), Some(param)) = (def.controls.get(index), def.desc.params.get(index)) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Drive => self.drive_s.set_target(real),
            Ctl::Tone => self.tone_s.set_target(real),
            Ctl::Level => self.level_s.set_target(real),
            Ctl::Low => self.low_s.set_target(real),
            Ctl::Mid => self.mid_s.set_target(real),
            Ctl::High => self.high_s.set_target(real),
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let [os_l, os_r] = &mut self.os;
        let [circuit_l, circuit_r] = &mut self.circuits[self.model];
        for (bl, br) in left.chunks_mut(CHUNK).zip(right.chunks_mut(CHUNK)) {
            let n = bl.len();
            // Knob trajectories, shared by both channels: tone / EQ at the
            // base rate, drive at the oversampled rate the shaper runs at.
            for v in &mut self.tone_traj[..n] {
                *v = self.tone_s.tick();
            }
            for v in &mut self.low_traj[..n] {
                *v = self.low_s.tick();
            }
            for v in &mut self.mid_traj[..n] {
                *v = self.mid_s.tick();
            }
            for v in &mut self.high_traj[..n] {
                *v = self.high_s.tick();
            }
            for v in &mut self.drive_traj[..4 * n] {
                *v = self.drive_s.tick();
            }
            let drive_traj = &self.drive_traj[..4 * n];
            os_l.process(bl, |b| circuit_l.shape(b, drive_traj));
            os_r.process(br, |b| circuit_r.shape(b, drive_traj));
            circuit_l.post(bl, &self.tone_traj[..n]);
            circuit_r.post(br, &self.tone_traj[..n]);
            circuit_l.eq(
                bl,
                &self.low_traj[..n],
                &self.mid_traj[..n],
                &self.high_traj[..n],
            );
            circuit_r.eq(
                br,
                &self.low_traj[..n],
                &self.mid_traj[..n],
                &self.high_traj[..n],
            );
            for (l, r) in bl.iter_mut().zip(br.iter_mut()) {
                let level = drive_law::level_lin(self.level_s.tick());
                *l *= level;
                *r *= level;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, process_in_blocks, rms, sine};

    const SR: u32 = 48_000;

    fn prepared(model: usize) -> Drive {
        let mut d = Drive::new();
        d.prepare(SR);
        d.select_pedal(model);
        d
    }

    /// Set a knob by position `0..=10` at the active pedal's param `index`.
    fn set_pos(d: &mut Drive, index: usize, pos: f32) {
        d.set_param(index, pos / 10.0);
    }

    /// The level/output knob is the last param on every faceplate.
    fn level_index(model: usize) -> usize {
        MODELS[model].desc.params.len() - 1
    }

    /// A sine at nominal guitar level (−18 dBFS). The character tests run
    /// here: full scale would drown every model in saturation and erase the
    /// differences the models exist for.
    fn guitar(freq: f32, len: usize) -> Vec<f32> {
        let mut x = sine(SR, freq, len);
        for s in &mut x {
            *s *= 0.126;
        }
        x
    }

    /// A quiet multi-tone probe: −30 dBFS sines at `freqs`, for EQ/voicing
    /// measurements. Probes must be *in* the signal — projecting onto a
    /// frequency the input doesn't contain reads the noise floor, not the
    /// response. Frequencies must be multiples of 2 Hz so every tone (and
    /// every intermod product) lands on an exact bin of the 0.5 s
    /// measurement tail, and no pairwise sum/difference of `freqs` may
    /// collide with another probe.
    fn tones(freqs: &[f32], len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            for f in freqs {
                *s += 0.03 * (std::f32::consts::TAU * f * t).sin();
            }
        }
        x
    }

    /// Magnitude of the projection onto `freq` over the settled tail.
    fn tone_at(y: &[f32], freq: f32) -> f64 {
        let tail = &y[y.len() / 2..];
        let n = tail.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in tail.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(freq) * i as f64 / f64::from(SR);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        ((cs * 2.0 / n).powi(2) + (cc * 2.0 / n).powi(2)).sqrt()
    }

    /// RMS fraction of the output that is *not* the fundamental — the
    /// distortion fingerprint used by the character tests.
    fn harmonic_residual(y: &[f32], f0: f32) -> f64 {
        let tail = &y[y.len() / 2..];
        let fund_rms = tone_at(y, f0) / 2f64.sqrt();
        let total_rms = f64::from(rms(tail));
        (total_rms.powi(2) - fund_rms.powi(2)).max(0.0).sqrt() / total_rms
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.pedals.len(), MODELS.len());
        for (def, desc) in MODELS.iter().zip(FAMILY.pedals) {
            assert!(std::ptr::eq(def.desc, *desc), "MODELS aligned with FAMILY");
            assert_eq!(def.controls.len(), def.desc.params.len());
        }
        // Keys are unique (they are REPL/preset-facing identifiers).
        for (i, a) in FAMILY.pedals.iter().enumerate() {
            for b in &FAMILY.pedals[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
        // The preset migrations reference pedals by index and key; pin them.
        let keys: Vec<&str> = FAMILY.pedals.iter().map(|p| p.key).collect();
        assert_eq!(keys, lh_core::preset::DRIVE_PEDALS);
        assert_eq!(
            FAMILY.pedals[lh_core::preset::CLASSIC_DRIVE_MODEL as usize].key,
            "classic"
        );
        // Each pedal wears its own faceplate — no inherited knobs.
        let captions =
            |i: usize| -> Vec<&str> { FAMILY.pedals[i].params.iter().map(|p| p.name).collect() };
        assert_eq!(captions(0), ["Drive", "Tone", "Level"]);
        assert_eq!(captions(1), ["Gain", "Tone", "Level"]);
        assert_eq!(captions(2), ["Drive", "Tone", "Level"]);
        assert_eq!(captions(3), ["Gain", "Treble", "Output"]);
        assert_eq!(captions(4), ["Gain", "Low", "Mid", "High", "Level"]);
        assert_eq!(captions(5), ["Gain", "Bass", "Middle", "Treble", "Master"]);
        assert_eq!(captions(6), ["Pre Gain", "Low", "Mid", "High", "Post Gain"]);
        assert_eq!(captions(7), ["Gain", "Bass", "Middle", "Treble", "Volume"]);
        assert_eq!(captions(8), ["Gain", "Bass", "Treble", "Volume"]);
        assert_eq!(captions(9), ["Fuzz", "Volume"]);
        assert_eq!(captions(10), ["Drive", "Tone", "Level"]);
        assert_eq!(captions(11), ["Drive", "Tone", "Level"]);
        assert_eq!(captions(12), ["Drive", "Tone", "Level"]);
        assert_eq!(captions(13), ["Gain", "Bass", "Middle", "Treble", "Volume"]);
        assert!(
            FAMILY.pedals[0].param_index("low").is_none(),
            "no EQ knobs on ts9"
        );
        assert!(
            FAMILY.pedals[4].param_index("tone").is_none(),
            "evva's dead tone knob is gone"
        );
        // Selection resolves by key or display name, case-insensitive.
        assert_eq!(FAMILY.pedal_index("bd2"), Some(1));
        assert_eq!(FAMILY.pedal_index("Blues Driver"), Some(1));
        assert_eq!(FAMILY.pedal_index("wah"), None);
    }

    #[test]
    fn every_model_is_finite_bounded_and_alive_at_max_drive() {
        for (model, def) in MODELS.iter().enumerate() {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 10.0);
            set_pos(&mut d, level_index(model), 10.0);
            let x = sine(SR, 220.0, SR as usize / 2);
            let y = process_in_blocks(&mut d, &x, 64);
            assert_finite(def.desc.key, &y);
            let p = peak(&y);
            // Full-scale input with every knob maxed: clippers saturate, and
            // the centaur's never-clipping clean path may legitimately ride
            // above full scale (level tops out at +9 dB) — bounded means "no
            // runaway", not "inside 0 dBFS".
            assert!(p < 4.5, "{}: bounded output, got peak {p}", def.desc.key);
            assert!(p > 0.2, "{}: signal present, got peak {p}", def.desc.key);
        }
    }

    #[test]
    fn every_model_removes_dc_and_dies_to_silence() {
        for (model, def) in MODELS.iter().enumerate() {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 9.0);
            let x = sine(SR, 220.0, SR as usize);
            let y = process_in_blocks(&mut d, &x, 256);
            let tail = &y[SR as usize / 2..];
            let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
            assert!(
                mean.abs() < 2e-3,
                "{}: DC must be blocked, mean {mean}",
                def.desc.key
            );

            d.reset();
            let silence = vec![0.0f32; SR as usize / 4];
            let y = process_in_blocks(&mut d, &silence, 128);
            let out = rms(&y[y.len() / 2..]);
            assert!(
                out < 1e-4,
                "{}: silence in → silence out, got rms {out}",
                def.desc.key
            );
        }
    }

    #[test]
    fn every_model_creates_harmonics() {
        // At nominal guitar level: character lives at real signal levels
        // (full scale would let the centaur's linear clean path drown its
        // saturated dirty path — the opposite of how the pedal is played).
        for (model, def) in MODELS.iter().enumerate() {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 7.0);
            let x = guitar(220.0, SR as usize);
            let y = process_in_blocks(&mut d, &x, 256);
            let residual = harmonic_residual(&y, 220.0);
            assert!(
                residual > 0.05,
                "{}: expected >5% harmonic content, got {residual:.3}",
                def.desc.key
            );
        }
    }

    #[test]
    fn ts9_clips_mids_harder_than_lows() {
        // The screamer signature: the gained path is high-passed at 720 Hz,
        // so a low note stays much cleaner than a mid note at the same knob.
        let mut d = prepared(0);
        set_pos(&mut d, 0, 6.0);
        let lows = {
            let x = guitar(110.0, SR as usize);
            harmonic_residual(&process_in_blocks(&mut d, &x, 256), 110.0)
        };
        d.reset();
        let mids = {
            let x = guitar(720.0, SR as usize);
            harmonic_residual(&process_in_blocks(&mut d, &x, 256), 720.0)
        };
        assert!(
            mids > 1.5 * lows,
            "ts9 must distort mids ≫ lows: mids {mids:.3} vs lows {lows:.3}"
        );
    }

    #[test]
    fn screamer_makes_the_mid_hump() {
        // The white-box TS is still a TS: the 720 Hz input high-pass keeps a
        // low note cleaner than a mid note at the same drive.
        let mut d = prepared(11);
        set_pos(&mut d, 0, 6.0);
        let lows = harmonic_residual(
            &process_in_blocks(&mut d, &guitar(110.0, SR as usize), 256),
            110.0,
        );
        d.reset();
        let mids = harmonic_residual(
            &process_in_blocks(&mut d, &guitar(720.0, SR as usize), 256),
            720.0,
        );
        // A gentler hump than the ts9's fb-shaped clipped path, but present:
        // the reactive clipper spreads breakup a touch more evenly.
        assert!(
            mids > 1.15 * lows,
            "screamer mid-hump: mids {mids:.3} vs lows {lows:.3}"
        );
    }

    #[test]
    fn screamer_clip_is_symmetric() {
        // Matched antiparallel 1N4148s: the 2nd harmonic stays small, like the
        // ts9 and unlike the asymmetric evva/bd2.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(11);
        set_pos(&mut d, 0, 6.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let h2 = tone_at(&y, 440.0) / tone_at(&y, 220.0);
        assert!(
            h2 < 0.02,
            "matched diodes → small 2nd harmonic, got {h2:.4}"
        );
    }

    #[test]
    fn screamer_voices_highs_differently_from_the_memoryless_ts9() {
        // The point of shipping both: the circuit model and the static curve
        // are genuinely different pedals, not a reskin. Up high, the WDF's
        // harder diode knee keeps more edge than the ts9's soft `x/√(1+x²)`
        // curve behind its aggressive 51 pF feedback lowpass — a clearly
        // measurable difference at the same drive. (The intrinsic frequency
        // dependence itself is pinned, unconfounded, at the WDF core in
        // `screamer::tests::shunt_cap_makes_clipping_frequency_dependent`.)
        let x = guitar(5_000.0, SR as usize);
        let mut sc = prepared(11);
        set_pos(&mut sc, 0, 8.0);
        let sc_res = harmonic_residual(&process_in_blocks(&mut sc, &x, 256), 5_000.0);
        let mut ts = prepared(0);
        set_pos(&mut ts, 0, 8.0);
        let ts_res = harmonic_residual(&process_in_blocks(&mut ts, &x, 256), 5_000.0);
        assert!(
            (sc_res - ts_res).abs() > 0.03,
            "circuit model must differ audibly from the curve: screamer {sc_res:.3} vs ts9 {ts_res:.3}"
        );
    }

    #[test]
    fn sd1_makes_even_harmonics_where_the_screamer_does_not() {
        // The SD-1's asymmetric feedback clipper (2 diodes vs 1) grows a strong
        // 2nd harmonic; the screamer's matched antiparallel pair cancels it —
        // the audible payoff of the feedback topology + asymmetric root.
        let x = guitar(220.0, SR as usize);
        let h2 = |y: &[f32]| tone_at(y, 440.0) / tone_at(y, 220.0);

        let mut sd1 = prepared(12);
        set_pos(&mut sd1, 0, 6.0);
        let sd1_h2 = h2(&process_in_blocks(&mut sd1, &x, 256));

        let mut sc = prepared(11);
        set_pos(&mut sc, 0, 6.0);
        let sc_h2 = h2(&process_in_blocks(&mut sc, &x, 256));

        assert!(
            sd1_h2 > 0.03,
            "sd1 asymmetric clipper — expected 2nd harmonic, got {sd1_h2:.4}"
        );
        assert!(
            sd1_h2 > 3.0 * sc_h2,
            "sd1 must out-even-harmonic the matched screamer: {sd1_h2:.4} vs {sc_h2:.4}"
        );
    }

    #[test]
    fn sd1_makes_the_mid_hump() {
        // Like every TS-family overdrive, a low note stays cleaner than a mid
        // note at the same drive — but here the hump is grown by the gain
        // leg's C_g (loop gain rolls off below its corner), not a hand-tuned
        // input high-pass the way the screamer fakes it.
        let mut d = prepared(12);
        set_pos(&mut d, 0, 6.0);
        let lows = harmonic_residual(
            &process_in_blocks(&mut d, &guitar(110.0, SR as usize), 256),
            110.0,
        );
        d.reset();
        let mids = harmonic_residual(
            &process_in_blocks(&mut d, &guitar(720.0, SR as usize), 256),
            720.0,
        );
        assert!(
            mids > 1.2 * lows,
            "sd1 mid-hump: mids {mids:.3} vs lows {lows:.3}"
        );
    }

    #[test]
    fn angry_charlie_v2_distorts_harder_than_the_v1() {
        // More gain, into distortion: at the same moderate drive the V2's
        // cascaded second clip leaves far less of the fundamental intact than
        // the V1's single crunch stage.
        let x = guitar(220.0, SR as usize);
        let mut v2 = prepared(13);
        set_pos(&mut v2, 0, 3.0);
        let v2_res = harmonic_residual(&process_in_blocks(&mut v2, &x, 256), 220.0);
        let mut v1 = prepared(7);
        set_pos(&mut v1, 0, 3.0);
        let v1_res = harmonic_residual(&process_in_blocks(&mut v1, &x, 256), 220.0);
        assert!(
            v2_res > 1.3 * v1_res,
            "angry-charlie-v2 must reach distortion past the v1 crunch: v2 {v2_res:.3} vs v1 {v1_res:.3}"
        );
    }

    #[test]
    fn angry_charlie_v2_lifts_the_mids_over_the_v1() {
        // The built-in 600–800 Hz boost: at a near-clean setting the V2 puts
        // more energy at 700 Hz relative to 250 Hz than the V1. Probed quiet so
        // the chain stays linear and the filter — not the clipping — shows.
        let x = tones(&[250.0, 700.0, 2_000.0], SR as usize);
        let tilt = |model: usize| -> f64 {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 1.0);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, 700.0) / tone_at(&y, 250.0)
        };
        let v2 = tilt(13);
        let v1 = tilt(7);
        assert!(
            v2 > 1.3 * v1,
            "angry-charlie-v2 must push the 700 Hz mids over the v1: v2 {v2:.3} vs v1 {v1:.3}"
        );
    }

    #[test]
    fn blues_driver_clips_lows_that_the_ts9_leaves_clean() {
        // Full-range vs mid-focused: at the same drive position, a 110 Hz
        // note comes out dirtier from the BD-2 than from the TS9.
        let x = guitar(110.0, SR as usize);
        let mut ts9 = prepared(0);
        set_pos(&mut ts9, 0, 7.0);
        let ts9_res = harmonic_residual(&process_in_blocks(&mut ts9, &x, 256), 110.0);
        let mut bd2 = prepared(1);
        set_pos(&mut bd2, 0, 7.0);
        let bd2_res = harmonic_residual(&process_in_blocks(&mut bd2, &x, 256), 110.0);
        assert!(
            bd2_res > 1.5 * ts9_res,
            "blues driver keeps (and clips) lows: bd2 {bd2_res:.3} vs ts9 {ts9_res:.3}"
        );
    }

    #[test]
    fn blues_driver_makes_even_harmonics_and_the_ts9_does_not() {
        // Asymmetric knees put energy at 2·f0; matched diodes cancel it.
        // (Peak ratios are useless here — the DC blocker recenters a
        // saturated waveform — the second harmonic is the honest metric.)
        let x = guitar(220.0, SR as usize);
        let h2 = |y: &[f32]| tone_at(y, 440.0) / tone_at(y, 220.0);

        let mut bd2 = prepared(1);
        set_pos(&mut bd2, 0, 6.0);
        let bd2_h2 = h2(&process_in_blocks(&mut bd2, &x, 256));

        let mut ts9 = prepared(0);
        set_pos(&mut ts9, 0, 6.0);
        let ts9_h2 = h2(&process_in_blocks(&mut ts9, &x, 256));

        assert!(
            bd2_h2 > 0.03,
            "bd2 knees are one diode against two — expected 2nd harmonic, got {bd2_h2:.4}"
        );
        assert!(
            ts9_h2 < 0.01,
            "ts9 diodes are matched — 2nd harmonic should vanish, got {ts9_h2:.4}"
        );
        assert!(bd2_h2 > 3.0 * ts9_h2);
    }

    #[test]
    fn modelled_pedals_sit_near_unity_at_default_knobs() {
        // Model switching mid-set must not blast the monitors: the modelled
        // pedals are calibrated to comparable loudness at their default
        // positions and nominal guitar level. (classic is exempt — its gain
        // structure is pinned by v1 preset compatibility.)
        let x = guitar(220.0, SR as usize);
        let in_rms = f64::from(rms(&x[x.len() / 2..]));
        let mut levels = Vec::new();
        for model in [0usize, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13] {
            let mut d = prepared(model);
            let y = process_in_blocks(&mut d, &x, 256);
            let out = f64::from(rms(&y[y.len() / 2..]));
            let db = 20.0 * (out / in_rms).log10();
            assert!(
                db.abs() < 6.0,
                "{}: default knobs should sit near unity, got {db:.1} dB",
                MODELS[model].desc.key
            );
            levels.push(db);
        }
        let spread = levels.iter().cloned().fold(f64::MIN, f64::max)
            - levels.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            spread < 5.0,
            "modelled pedals at defaults are {spread:.1} dB apart ({levels:?})"
        );
    }

    #[test]
    fn centaur_low_gain_is_a_transparent_boost() {
        // The Klon party trick: with the gain low, the clean path dominates
        // — barely any harmonics, level near (or a touch above) unity.
        let x = guitar(220.0, SR as usize);
        let mut c = prepared(3);
        set_pos(&mut c, 0, 1.5);
        let y = process_in_blocks(&mut c, &x, 256);
        let residual = harmonic_residual(&y, 220.0);
        assert!(
            residual < 0.04,
            "low-gain centaur must stay clean, got {residual:.3} residual"
        );
        let db =
            20.0 * (f64::from(rms(&y[y.len() / 2..])) / f64::from(rms(&x[x.len() / 2..]))).log10();
        assert!(
            (-3.0..6.0).contains(&db),
            "transparent boost, not a cut: {db:.1} dB"
        );

        // And the same pedal still breaks up when pushed.
        let mut c = prepared(3);
        set_pos(&mut c, 0, 9.0);
        let y = process_in_blocks(&mut c, &x, 256);
        let pushed = harmonic_residual(&y, 220.0);
        assert!(
            pushed > 2.0 * residual.max(0.01),
            "cranked centaur must break up: {pushed:.3}"
        );
    }

    #[test]
    fn model_switch_mid_note_stays_finite() {
        let mut d = prepared(0);
        set_pos(&mut d, 0, 8.0);
        let x = sine(SR, 220.0, SR as usize / 2);
        let mut left = x.clone();
        let mut right = x.clone();
        for (i, (bl, br)) in left.chunks_mut(64).zip(right.chunks_mut(64)).enumerate() {
            // Cycle through every pedal while the note rings.
            d.select_pedal(i % MODEL_COUNT);
            d.process(bl, br);
        }
        assert_finite("model switching", &left);
        assert!(peak(&left) < 4.0);
    }

    #[test]
    fn param_changes_are_smooth() {
        for (model, def) in MODELS.iter().enumerate() {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 0.0);
            let x = sine(SR, 220.0, SR as usize / 2);
            let mut y = x.clone();
            let mut yr = x.clone();
            let (a, b) = y.split_at_mut(SR as usize / 4);
            let (ar, br) = yr.split_at_mut(SR as usize / 4);
            d.process(a, ar);
            d.set_param(0, 1.0); // slam the knob mid-stream
            d.set_param(level_index(model), 0.0);
            d.process(b, br);
            assert_finite("drive sweep", &y);
            let max_step = y
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max);
            // 220 Hz at unity swings ~0.03/sample; a hard gain jump spikes this.
            assert!(
                max_step < 0.5,
                "{}: click detected, step {max_step}",
                def.desc.key
            );
        }
    }

    #[test]
    fn all_models_run_at_studio_rates() {
        for sr in [44_100u32, 96_000] {
            for (model, def) in MODELS.iter().enumerate() {
                let mut d = Drive::new();
                d.prepare(sr);
                d.select_pedal(model);
                set_pos(&mut d, 0, 7.0);
                let x = sine(sr, 220.0, sr as usize / 2);
                let y = process_in_blocks(&mut d, &x, 96);
                assert_finite(def.desc.key, &y);
                assert!(rms(&y[y.len() / 2..]) > 1e-3);
            }
        }
    }

    #[test]
    fn evva_creates_even_harmonics() {
        // Asymmetric knees (0.8 / 0.5) produce a healthy 2nd harmonic.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(4);
        set_pos(&mut d, 0, 6.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let h2 = tone_at(&y, 440.0) / tone_at(&y, 220.0);
        assert!(h2 > 0.03, "evva even harmonics: h2/f0 = {h2:.4}");
        let h3 = tone_at(&y, 660.0) / tone_at(&y, 220.0);
        assert!(h3 > 0.01, "evva odd harmonics: h3/f0 = {h3:.4}");
    }

    #[test]
    fn evva_low_gain_is_clean() {
        // Below gain 3 the evva is mostly a clean boost.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(4);
        set_pos(&mut d, 0, 2.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let residual = harmonic_residual(&y, 220.0);
        assert!(
            residual < 0.08,
            "evva low-gain must stay clean, got {residual:.3} residual"
        );
    }

    #[test]
    fn evva_eq_bands_work() {
        // Verify each EQ band actually shapes the spectrum, probing with
        // tones the input actually contains (see `tones`).
        let x = tones(&[80.0, 200.0, 500.0, 750.0, 3_000.0, 6_100.0], SR as usize);
        let measure = |low: f32, mid: f32, high: f32, freq: f32| -> f64 {
            let mut d = prepared(4);
            set_pos(&mut d, 0, 2.0); // low gain — EQ dominates
            set_pos(&mut d, 1, low);
            set_pos(&mut d, 2, mid);
            set_pos(&mut d, 3, high);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, freq)
        };

        // Low shelf: boosting low should lift 80 Hz relative to 3 kHz.
        let flat_lo = measure(5.0, 5.0, 5.0, 80.0) / measure(5.0, 5.0, 5.0, 3_000.0);
        let boosted_lo = measure(10.0, 5.0, 5.0, 80.0) / measure(10.0, 5.0, 5.0, 3_000.0);
        assert!(
            boosted_lo > 1.3 * flat_lo,
            "low shelf boost: {boosted_lo:.3} vs flat {flat_lo:.3}"
        );

        // Mid band: boosting mid should lift 750 Hz relative to 200 Hz.
        let flat_mid = measure(5.0, 5.0, 5.0, 750.0) / measure(5.0, 5.0, 5.0, 200.0);
        let boosted_mid = measure(5.0, 10.0, 5.0, 750.0) / measure(5.0, 10.0, 5.0, 200.0);
        assert!(
            boosted_mid > 1.5 * flat_mid,
            "mid boost: {boosted_mid:.3} vs flat {flat_mid:.3}"
        );

        // High shelf: boosting high should lift 6.1 kHz relative to 500 Hz.
        let flat_hi = measure(5.0, 5.0, 5.0, 6_100.0) / measure(5.0, 5.0, 5.0, 500.0);
        let boosted_hi = measure(5.0, 5.0, 10.0, 6_100.0) / measure(5.0, 5.0, 10.0, 500.0);
        assert!(
            boosted_hi > 1.5 * flat_hi,
            "high shelf boost: {boosted_hi:.3} vs flat {flat_hi:.3}"
        );

        // Cut: low at 0 should reduce 80 Hz.
        let cut_lo = measure(0.0, 5.0, 5.0, 80.0) / measure(0.0, 5.0, 5.0, 3_000.0);
        assert!(
            cut_lo < 0.7 * flat_lo,
            "low shelf cut: {cut_lo:.3} vs flat {flat_lo:.3}"
        );
    }

    #[test]
    fn red_charlie_distorts_harder_than_the_screamer() {
        // A distortion, not an overdrive: the cascaded stages leave far less
        // of the fundamental intact than the dry-summing TS9 at the same
        // knob position.
        let x = guitar(220.0, SR as usize);
        let mut jcm = prepared(5);
        set_pos(&mut jcm, 0, 6.0);
        let jcm_res = harmonic_residual(&process_in_blocks(&mut jcm, &x, 256), 220.0);
        let mut ts9 = prepared(0);
        set_pos(&mut ts9, 0, 6.0);
        let ts9_res = harmonic_residual(&process_in_blocks(&mut ts9, &x, 256), 220.0);
        assert!(
            jcm_res > 1.5 * ts9_res,
            "red-charlie cascade must out-distort the ts9: {jcm_res:.3} vs {ts9_res:.3}"
        );
    }

    #[test]
    fn red_charlie_cold_clipper_makes_even_harmonics() {
        // The cold-biased second stage shears one polarity early — a strong
        // 2nd harmonic is the fingerprint.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(5);
        set_pos(&mut d, 0, 6.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let h2 = tone_at(&y, 440.0) / tone_at(&y, 220.0);
        assert!(h2 > 0.03, "red-charlie even harmonics: h2/f0 = {h2:.4}");
    }

    #[test]
    fn red_charlie_voicing_thins_the_lows_before_the_gain() {
        // The tight palm-mute low end: the cathode-network trim and the
        // interstage coupling pull ~8 dB out of a 110 Hz note relative to
        // the mids *before* the hot stage. Measured at low gain where the
        // voicing is linear.
        let x = tones(&[110.0, 650.0], SR as usize);
        let mut d = prepared(5);
        set_pos(&mut d, 0, 1.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let tilt_in = tone_at(&x, 650.0) / tone_at(&x, 110.0);
        let tilt_out = tone_at(&y, 650.0) / tone_at(&y, 110.0);
        assert!(
            tilt_out > 1.8 * tilt_in,
            "red-charlie must thin lows against mids: out {tilt_out:.3} vs in {tilt_in:.3}"
        );
    }

    #[test]
    fn red_charlie_low_gain_keeps_the_bright_cap_edge() {
        // With the preamp pot low the bright cap dominates: a treble tone
        // gains noticeably more than a low-mid tone through the (nearly
        // clean) pedal.
        let x = tones(&[300.0, 3_000.0], SR as usize);
        let mut d = prepared(5);
        set_pos(&mut d, 0, 1.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let tilt_in = tone_at(&x, 3_000.0) / tone_at(&x, 300.0);
        let tilt_out = tone_at(&y, 3_000.0) / tone_at(&y, 300.0);
        assert!(
            tilt_out > 1.3 * tilt_in,
            "bright cap must tilt low-gain treble up: out {tilt_out:.3} vs in {tilt_in:.3}"
        );
    }

    #[test]
    fn red_charlie_eq_bands_work() {
        // Verify each tone-stack band actually shapes the spectrum. The
        // probe tones are in the input, and between measurements only EQ
        // knobs change — the pre-EQ signal is identical, so ratios read the
        // stack's true response even though the cascade clips upstream.
        let x = tones(&[70.0, 150.0, 400.0, 650.0, 3_000.0, 6_100.0], SR as usize);
        let measure = |bass: f32, middle: f32, treble: f32, freq: f32| -> f64 {
            let mut d = prepared(5);
            set_pos(&mut d, 0, 2.0);
            set_pos(&mut d, 1, bass);
            set_pos(&mut d, 2, middle);
            set_pos(&mut d, 3, treble);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, freq)
        };

        // Bass shelf: boosting bass should lift 70 Hz relative to 3 kHz.
        let flat_lo = measure(5.0, 5.0, 5.0, 70.0) / measure(5.0, 5.0, 5.0, 3_000.0);
        let boosted_lo = measure(10.0, 5.0, 5.0, 70.0) / measure(10.0, 5.0, 5.0, 3_000.0);
        assert!(
            boosted_lo > 1.3 * flat_lo,
            "bass boost: {boosted_lo:.3} vs flat {flat_lo:.3}"
        );

        // Middle: boosting should lift 650 Hz relative to 150 Hz.
        let flat_mid = measure(5.0, 5.0, 5.0, 650.0) / measure(5.0, 5.0, 5.0, 150.0);
        let boosted_mid = measure(5.0, 10.0, 5.0, 650.0) / measure(5.0, 10.0, 5.0, 150.0);
        assert!(
            boosted_mid > 1.5 * flat_mid,
            "middle boost: {boosted_mid:.3} vs flat {flat_mid:.3}"
        );

        // Treble shelf: boosting should lift 6.1 kHz relative to 400 Hz.
        let flat_hi = measure(5.0, 5.0, 5.0, 6_100.0) / measure(5.0, 5.0, 5.0, 400.0);
        let boosted_hi = measure(5.0, 5.0, 10.0, 6_100.0) / measure(5.0, 5.0, 10.0, 400.0);
        assert!(
            boosted_hi > 1.5 * flat_hi,
            "treble boost: {boosted_hi:.3} vs flat {flat_hi:.3}"
        );

        // Cut: middle at 0 should scoop 650 Hz — the metal preset.
        let cut_mid = measure(5.0, 0.0, 5.0, 650.0) / measure(5.0, 0.0, 5.0, 150.0);
        assert!(
            cut_mid < 0.7 * flat_mid,
            "middle scoop: {cut_mid:.3} vs flat {flat_mid:.3}"
        );
    }

    #[test]
    fn monster5150_sustains_where_the_red_charlie_decays() {
        // The high-gain signature is compression, not a bigger residual —
        // driven hard, both cascades sit on the square-wave ceiling (the
        // red-charlie's extended solo pot reaches it too). The designs part
        // ways at *low* knobs: feed a note tail (−38 dBFS) with both gains
        // at 2 — the red-charlie cleans up and lets the tail decay, the
        // monster's clean-free floor keeps slamming the third stage.
        let mut x = guitar(220.0, SR as usize);
        for s in &mut x {
            *s *= 0.1;
        }
        let mut monster = prepared(6);
        set_pos(&mut monster, 0, 2.0);
        let monster_rms = f64::from(rms(
            &process_in_blocks(&mut monster, &x, 256)[SR as usize / 2..]
        ));
        let mut red = prepared(5);
        set_pos(&mut red, 0, 2.0);
        let red_rms = f64::from(rms(&process_in_blocks(&mut red, &x, 256)[SR as usize / 2..]));
        assert!(
            monster_rms > 1.4 * red_rms,
            "monster5150 must sustain the tail harder: {monster_rms:.4} vs {red_rms:.4}"
        );
    }

    #[test]
    fn monster5150_has_no_clean() {
        // The lead channel's gain floor still crunches: even at pre gain 1
        // the cascade is audibly saturated.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(6);
        set_pos(&mut d, 0, 1.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let residual = harmonic_residual(&y, 220.0);
        assert!(
            residual > 0.12,
            "monster5150 must stay dirty at minimum gain, got {residual:.3}"
        );
    }

    #[test]
    fn monster5150_trims_lows_before_the_gain() {
        // Chug tightness: the input trim plus the double interstage
        // coupling thin a 110 Hz note against the mids ahead of the
        // cascade. Probed ~30 dB below guitar level so all three stages
        // stay linear even with the pre gain at its (dirty) floor.
        let mut x = tones(&[110.0, 550.0], SR as usize);
        for s in &mut x {
            *s *= 0.066;
        }
        let mut d = prepared(6);
        set_pos(&mut d, 0, 0.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let tilt_in = tone_at(&x, 550.0) / tone_at(&x, 110.0);
        let tilt_out = tone_at(&y, 550.0) / tone_at(&y, 110.0);
        assert!(
            tilt_out > 1.8 * tilt_in,
            "monster5150 must thin lows against mids: out {tilt_out:.3} vs in {tilt_in:.3}"
        );
    }

    #[test]
    fn monster5150_eq_bands_work() {
        // Same identity trick as the red-charlie's EQ test: only EQ knobs
        // change between measurements, so ratios read the post-distortion
        // stack's true response.
        let x = tones(&[70.0, 150.0, 420.0, 550.0, 3_000.0, 6_100.0], SR as usize);
        let measure = |low: f32, mid: f32, high: f32, freq: f32| -> f64 {
            let mut d = prepared(6);
            set_pos(&mut d, 0, 2.0);
            set_pos(&mut d, 1, low);
            set_pos(&mut d, 2, mid);
            set_pos(&mut d, 3, high);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, freq)
        };

        // Low shelf: boosting low should lift 70 Hz relative to 3 kHz —
        // the resonance-style thickness dialed in after the clipping.
        let flat_lo = measure(5.0, 5.0, 5.0, 70.0) / measure(5.0, 5.0, 5.0, 3_000.0);
        let boosted_lo = measure(10.0, 5.0, 5.0, 70.0) / measure(10.0, 5.0, 5.0, 3_000.0);
        assert!(
            boosted_lo > 1.3 * flat_lo,
            "low boost: {boosted_lo:.3} vs flat {flat_lo:.3}"
        );

        // Mid: boosting should lift 550 Hz relative to 150 Hz.
        let flat_mid = measure(5.0, 5.0, 5.0, 550.0) / measure(5.0, 5.0, 5.0, 150.0);
        let boosted_mid = measure(5.0, 10.0, 5.0, 550.0) / measure(5.0, 10.0, 5.0, 150.0);
        assert!(
            boosted_mid > 1.5 * flat_mid,
            "mid boost: {boosted_mid:.3} vs flat {flat_mid:.3}"
        );

        // High shelf: boosting should lift 6.1 kHz relative to 420 Hz.
        let flat_hi = measure(5.0, 5.0, 5.0, 6_100.0) / measure(5.0, 5.0, 5.0, 420.0);
        let boosted_hi = measure(5.0, 5.0, 10.0, 6_100.0) / measure(5.0, 5.0, 10.0, 420.0);
        assert!(
            boosted_hi > 1.5 * flat_hi,
            "high boost: {boosted_hi:.3} vs flat {flat_hi:.3}"
        );

        // Cut: mid at 0 is the scooped-wall preset.
        let cut_mid = measure(5.0, 0.0, 5.0, 550.0) / measure(5.0, 0.0, 5.0, 150.0);
        assert!(
            cut_mid < 0.7 * flat_mid,
            "mid scoop: {cut_mid:.3} vs flat {flat_mid:.3}"
        );
    }

    #[test]
    fn monster5150_clip_is_symmetric_and_suppresses_even_harmonics() {
        // Re-tooled to matched diode-to-ground clamps at every stage — the
        // even harmonics the old asymmetric cascade made should be starved,
        // same fingerprint as the angry-charlie's LEDs and a sharp contrast
        // with the red-charlie's cold-biased (asymmetric) cascade.
        let x = guitar(220.0, SR as usize);
        let mut monster = prepared(6);
        set_pos(&mut monster, 0, 6.0);
        let monster_h2 = {
            let y = process_in_blocks(&mut monster, &x, 256);
            tone_at(&y, 440.0) / tone_at(&y, 220.0)
        };
        assert!(
            monster_h2 < 0.02,
            "monster5150's symmetric clip should suppress even harmonics, got h2/f0 = {monster_h2:.4}"
        );
        let mut red = prepared(5);
        set_pos(&mut red, 0, 6.0);
        let red_h2 = {
            let y = process_in_blocks(&mut red, &x, 256);
            tone_at(&y, 440.0) / tone_at(&y, 220.0)
        };
        assert!(
            red_h2 > 3.0 * monster_h2,
            "monster5150 must suppress even harmonics far more than the red-charlie's \
             asymmetric cascade: monster {monster_h2:.4} vs red {red_h2:.4}"
        );
    }

    #[test]
    fn monster5150_cascades_harder_than_the_single_stage_angry_charlie() {
        // Three cascaded symmetric clamps (with interstage reshaping between
        // each) should out-saturate the angry-charlie's single hard clip at
        // the same knob position — the "one level deeper" cascade the model
        // is built around still holds once both are symmetric.
        let x = guitar(220.0, SR as usize);
        let mut monster = prepared(6);
        set_pos(&mut monster, 0, 4.0);
        let monster_res = harmonic_residual(&process_in_blocks(&mut monster, &x, 256), 220.0);
        let mut angry = prepared(7);
        set_pos(&mut angry, 0, 4.0);
        let angry_res = harmonic_residual(&process_in_blocks(&mut angry, &x, 256), 220.0);
        assert!(
            monster_res > angry_res,
            "monster5150's cascade must out-distort the angry-charlie's single clip: \
             monster {monster_res:.3} vs angry {angry_res:.3}"
        );
    }

    #[test]
    fn angry_charlie_clip_is_symmetric_and_suppresses_even_harmonics() {
        // Two matched red LEDs to ground — same fingerprint as the ts9's
        // matched feedback diodes (h2 should vanish), but from a hard knee
        // instead of a soft one.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(7);
        set_pos(&mut d, 0, 6.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let h2 = tone_at(&y, 440.0) / tone_at(&y, 220.0);
        assert!(
            h2 < 0.02,
            "angry-charlie's symmetric clip should suppress even harmonics, got h2/f0 = {h2:.4}"
        );
    }

    #[test]
    fn angry_charlie_hard_clip_is_squarer_than_the_red_charlies_soft_knee() {
        // Diodes to ground outside the feedback loop clamp flat; the
        // red-charlie's cascaded tanh knees round the top off instead.
        // Driven hard, the flat clamp's crest factor (peak/rms) sits closer
        // to a square wave's ~1.0 than the tanh cascade's.
        let x = guitar(220.0, SR as usize);
        let crest = |model: usize| -> f64 {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 9.0);
            let y = process_in_blocks(&mut d, &x, 256);
            let tail = &y[y.len() / 2..];
            f64::from(peak(tail)) / f64::from(rms(tail))
        };
        let angry_crest = crest(7);
        let red_crest = crest(5);
        assert!(
            angry_crest < red_crest,
            "angry-charlie's hard clip should be squarer (lower crest factor) than \
             red-charlie's soft knee: {angry_crest:.3} vs {red_crest:.3}"
        );
    }

    #[test]
    fn angry_charlie_stays_clean_below_the_led_threshold() {
        // Real LEDs don't conduct until well past a silicon diode's forward
        // voltage: low gain rides under the knee almost entirely clean,
        // unlike a continuously-compressing tanh stage.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(7);
        set_pos(&mut d, 0, 1.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let residual = harmonic_residual(&y, 220.0);
        assert!(
            residual < 0.05,
            "angry-charlie low-gain must stay clean under the LED knee, got {residual:.3} residual"
        );
    }

    #[test]
    fn angry_charlie_eq_bands_work() {
        // Same identity trick as the other post-distortion stacks: only EQ
        // knobs change between measurements.
        let x = tones(&[70.0, 150.0, 350.0, 550.0, 2_800.0, 6_000.0], SR as usize);
        let measure = |bass: f32, middle: f32, treble: f32, freq: f32| -> f64 {
            let mut d = prepared(7);
            set_pos(&mut d, 0, 2.0);
            set_pos(&mut d, 1, bass);
            set_pos(&mut d, 2, middle);
            set_pos(&mut d, 3, treble);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, freq)
        };

        // Bass shelf: boosting bass should lift 70 Hz relative to 2.8 kHz.
        let flat_lo = measure(5.0, 5.0, 5.0, 70.0) / measure(5.0, 5.0, 5.0, 2_800.0);
        let boosted_lo = measure(10.0, 5.0, 5.0, 70.0) / measure(10.0, 5.0, 5.0, 2_800.0);
        assert!(
            boosted_lo > 1.3 * flat_lo,
            "bass boost: {boosted_lo:.3} vs flat {flat_lo:.3}"
        );

        // Middle: boosting should lift 550 Hz relative to 150 Hz.
        let flat_mid = measure(5.0, 5.0, 5.0, 550.0) / measure(5.0, 5.0, 5.0, 150.0);
        let boosted_mid = measure(5.0, 10.0, 5.0, 550.0) / measure(5.0, 10.0, 5.0, 150.0);
        assert!(
            boosted_mid > 1.5 * flat_mid,
            "middle boost: {boosted_mid:.3} vs flat {flat_mid:.3}"
        );

        // Treble shelf: boosting should lift 6 kHz relative to 350 Hz.
        let flat_hi = measure(5.0, 5.0, 5.0, 6_000.0) / measure(5.0, 5.0, 5.0, 350.0);
        let boosted_hi = measure(5.0, 5.0, 10.0, 6_000.0) / measure(5.0, 5.0, 10.0, 350.0);
        assert!(
            boosted_hi > 1.5 * flat_hi,
            "treble boost: {boosted_hi:.3} vs flat {flat_hi:.3}"
        );

        // Cut: middle at 0 should scoop 550 Hz.
        let cut_mid = measure(5.0, 0.0, 5.0, 550.0) / measure(5.0, 0.0, 5.0, 150.0);
        assert!(
            cut_mid < 0.7 * flat_mid,
            "middle scoop: {cut_mid:.3} vs flat {flat_mid:.3}"
        );
    }

    #[test]
    fn evva_eq_is_flat_at_defaults() {
        // With every EQ knob at 5 the stack contributes 0 dB per band: the
        // output must match a run with the EQ hook skipped, bit for bit
        // aside from float noise. (The stack's flat gains are exactly
        // db_to_lin(0) − 1 = 0, so its adds vanish.)
        let x = tones(&[80.0, 220.0, 750.0, 3_000.0], SR as usize / 2);
        let mut d = prepared(4);
        set_pos(&mut d, 0, 2.0);
        let flat = process_in_blocks(&mut d, &x, 256);
        for freq in [80.0, 220.0, 750.0, 3_000.0] {
            let ratio = tone_at(&flat, freq) / tone_at(&x, freq).max(1e-12);
            // Near-clean gain 2 through makeup and level: each probe tone
            // must come through at a sane, roughly uniform level.
            assert!(
                (0.2..5.0).contains(&ratio),
                "flat evva EQ must pass {freq} Hz cleanly, ratio {ratio:.3}"
            );
        }
    }

    #[test]
    fn jan_ray_stays_dynamic_at_low_gain() {
        // The series-diode headroom: at gain 2 the Jan Ray is a near-clean,
        // touch-sensitive boost, barely into the clip — the transparent voice
        // it's famous for.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(8);
        set_pos(&mut d, 0, 2.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let residual = harmonic_residual(&y, 220.0);
        assert!(
            residual < 0.06,
            "jan-ray low-gain must stay clean, got {residual:.3} residual"
        );

        // And it still breaks up when pushed.
        let mut d = prepared(8);
        set_pos(&mut d, 0, 9.0);
        let pushed = harmonic_residual(&process_in_blocks(&mut d, &x, 256), 220.0);
        assert!(
            pushed > 2.0 * residual.max(0.01),
            "cranked jan-ray must break up: {pushed:.3}"
        );
    }

    #[test]
    fn jan_ray_distorts_lows_more_evenly_than_the_ts9() {
        // Full-range vs mid-scooped. Both pedals dirty a mid note; the
        // question is the *low* note. Only a 70 Hz subsonic trim sits ahead of
        // the Jan Ray's clip, so a 110 Hz note breaks up nearly as hard as an
        // 800 Hz one — the amp-in-a-box. The ts9's 720 Hz input high-pass
        // keeps its lows far cleaner than its mids: a much more lopsided
        // mid-over-low distortion ratio.
        let ratio = |model: usize| -> f64 {
            let mut d = prepared(model);
            // Measured at moderate gain, where the ts9's scoop is starkest:
            // cranked, even its attenuated lows eventually distort.
            set_pos(&mut d, 0, 4.0);
            let lows = harmonic_residual(
                &process_in_blocks(&mut d, &guitar(110.0, SR as usize), 256),
                110.0,
            );
            d.reset();
            let mids = harmonic_residual(
                &process_in_blocks(&mut d, &guitar(800.0, SR as usize), 256),
                800.0,
            );
            mids / lows.max(1e-6)
        };
        let jan = ratio(8);
        let ts9 = ratio(0);
        assert!(
            ts9 > 1.5 * jan,
            "ts9 must scoop lows harder than the jan-ray: ts9 mid/low {ts9:.2} vs jan {jan:.2}"
        );
    }

    #[test]
    fn jan_ray_chimes_brighter_than_its_input() {
        // The Fender sparkle: the fixed bright pre-emphasis tilts a treble
        // tone up relative to a low-mid tone through the (near-clean) pedal.
        // Measured at low gain where the voicing is linear.
        let x = tones(&[300.0, 4_000.0], SR as usize);
        let mut d = prepared(8);
        set_pos(&mut d, 0, 1.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let tilt_in = tone_at(&x, 4_000.0) / tone_at(&x, 300.0);
        let tilt_out = tone_at(&y, 4_000.0) / tone_at(&y, 300.0);
        assert!(
            tilt_out > 1.2 * tilt_in,
            "jan-ray chime must tilt treble up: out {tilt_out:.3} vs in {tilt_in:.3}"
        );
    }

    #[test]
    fn jan_ray_has_gentle_even_harmonic_warmth() {
        // The internal bias trim, modelled as mildly uneven knees (0.95/0.80),
        // leaves a small but real 2nd harmonic — the "tube-like" warmth — well
        // under the strong even harmonic of the openly-asymmetric evva.
        let x = guitar(220.0, SR as usize);
        let h2 = |model: usize, pos: f32| -> f64 {
            let mut d = prepared(model);
            set_pos(&mut d, 0, pos);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, 440.0) / tone_at(&y, 220.0)
        };
        let jan_h2 = h2(8, 6.0);
        assert!(
            jan_h2 > 0.015,
            "jan-ray bias asymmetry: expected a gentle 2nd harmonic, got {jan_h2:.4}"
        );
        assert!(
            jan_h2 < h2(4, 6.0),
            "jan-ray warmth must stay milder than the evva's strong even harmonic"
        );
    }

    #[test]
    fn jan_ray_tone_bands_work() {
        // The two-band Fender tone: a 120 Hz bass shelf and a 2.8 kHz treble
        // shelf (no mid). Same identity trick as the other post-distortion
        // stacks — only tone knobs change between measurements.
        let x = tones(&[70.0, 200.0, 500.0, 6_100.0], SR as usize);
        let measure = |bass: f32, treble: f32, freq: f32| -> f64 {
            let mut d = prepared(8);
            set_pos(&mut d, 0, 2.0);
            set_pos(&mut d, 1, bass);
            set_pos(&mut d, 2, treble);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, freq)
        };

        // Bass shelf: boosting bass lifts 70 Hz relative to 500 Hz.
        let flat_lo = measure(5.0, 5.0, 70.0) / measure(5.0, 5.0, 500.0);
        let boosted_lo = measure(10.0, 5.0, 70.0) / measure(10.0, 5.0, 500.0);
        assert!(
            boosted_lo > 1.3 * flat_lo,
            "bass boost: {boosted_lo:.3} vs flat {flat_lo:.3}"
        );

        // Treble shelf: boosting treble lifts 6.1 kHz relative to 500 Hz.
        let flat_hi = measure(5.0, 5.0, 6_100.0) / measure(5.0, 5.0, 500.0);
        let boosted_hi = measure(5.0, 10.0, 6_100.0) / measure(5.0, 10.0, 500.0);
        assert!(
            boosted_hi > 1.5 * flat_hi,
            "treble boost: {boosted_hi:.3} vs flat {flat_hi:.3}"
        );

        // Cut: bass at 0 thins 70 Hz — the tight, cranked setting.
        let cut_lo = measure(0.0, 5.0, 70.0) / measure(0.0, 5.0, 500.0);
        assert!(
            cut_lo < 0.7 * flat_lo,
            "bass cut: {cut_lo:.3} vs flat {flat_lo:.3}"
        );
    }

    /// A low two-note chord (112 + 176 Hz) at nominal guitar level. Their
    /// 64 Hz difference tone is absent from the input, so any energy there is
    /// pure nonlinear intermodulation — the "boom" of drive stacking.
    fn low_chord(len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            *s = 0.05 * (std::f32::consts::TAU * 112.0 * t).sin()
                + 0.05 * (std::f32::consts::TAU * 176.0 * t).sin();
        }
        x
    }

    #[test]
    fn centaur_boost_is_mid_forward() {
        // The real Klon tightens the low end — as a boost it must not lift the
        // lows as much as the mids, or stacked in front of a high-gain pedal
        // the bass farts out in the next clipper. Probe 80 Hz vs 800 Hz with
        // the Centaur set as a clean boost (drive 10%, output 100%).
        let x = tones(&[80.0, 800.0], SR as usize);
        let mut c = prepared(3);
        set_pos(&mut c, 0, 1.0);
        set_pos(&mut c, 2, 10.0);
        let y = process_in_blocks(&mut c, &x, 256);
        let tilt_in = tone_at(&x, 80.0) / tone_at(&x, 800.0);
        let tilt_out = tone_at(&y, 80.0) / tone_at(&y, 800.0);
        assert!(
            tilt_out < 0.8 * tilt_in,
            "centaur boost must tighten lows (mid-forward): out {tilt_out:.3} vs in {tilt_in:.3}"
        );
    }

    #[test]
    fn drive_stacking_stays_tight() {
        // The integration guard for drive stacking. A low chord (112 + 176 Hz)
        // has an odd-order intermod at 2·112−176 = 48 Hz — deep sub-bass,
        // absent from the input, generated only by clipping: the boom. Putting
        // a Centaur boost (drive 10%, output 100%) in front of an Angry Charlie
        // must not raise that sub-bass above the Angry Charlie played alone —
        // i.e. a mid-forward boost pours no extra bass into the clipper. (Before
        // the Centaur low-shelf and the pedals' tightening high-passes, the
        // boost lifted this intermod ~25 % over solo — the reported "boom".)
        let x = low_chord(SR as usize);
        let mut angry = prepared(7);
        let solo = process_in_blocks(&mut angry, &x, 256);

        let mut centaur = prepared(3);
        set_pos(&mut centaur, 0, 1.0); // drive 10%
        set_pos(&mut centaur, 2, 10.0); // output 100%
        let mut angry2 = prepared(7);
        let mid = process_in_blocks(&mut centaur, &x, 256);
        let stacked = process_in_blocks(&mut angry2, &mid, 256);

        let sub = |y: &[f32]| tone_at(y, 48.0);
        assert!(
            sub(&stacked) <= 1.15 * sub(&solo),
            "stacked boost must not add sub-bass boom: stacked {:.5} vs solo {:.5}",
            sub(&stacked),
            sub(&solo)
        );
    }

    /// A plucked note: a sine under an exponential decay envelope, at nominal
    /// guitar level. `tau` is the decay time constant in seconds.
    fn plucked(freq: f32, tau: f32, len: usize) -> Vec<f32> {
        let mut x = sine(SR, freq, len);
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            *s *= 0.126 * (-t / tau).exp();
        }
        x
    }

    #[test]
    fn fuzz_face_gates_the_decay() {
        // The germanium signature: blocking distortion cuts the note off as it
        // fades, so the late tail collapses to near-silence — where the ts9
        // (a plain clipper) compresses the same decay and keeps ringing. The
        // metric is tail-energy relative to body-energy: the fuzz's is far
        // smaller.
        let x = plucked(220.0, 0.25, SR as usize);
        let tail_over_body = |model: usize| -> f64 {
            let mut d = prepared(model);
            let y = process_in_blocks(&mut d, &x, 256);
            let body = f64::from(rms(&y[3 * y.len() / 8..y.len() / 2]));
            let tail = f64::from(rms(&y[7 * y.len() / 8..]));
            tail / body.max(1e-9)
        };
        let fuzz = tail_over_body(9);
        let ts9 = tail_over_body(0);
        assert!(
            ts9 > 3.0 * fuzz,
            "fuzz-face must gate its decay far harder than the ts9 sustains: \
             fuzz tail/body {fuzz:.4} vs ts9 {ts9:.4}"
        );
    }

    #[test]
    fn fuzz_face_is_strongly_asymmetric() {
        // One transistor saturates soft, the other cuts off hard: a big even
        // harmonic, unlike the ts9's matched (symmetric, even-starved) diodes.
        let x = guitar(220.0, SR as usize);
        let h2 = |model: usize| -> f64 {
            let mut d = prepared(model);
            set_pos(&mut d, 0, 6.0);
            let y = process_in_blocks(&mut d, &x, 256);
            tone_at(&y, 440.0) / tone_at(&y, 220.0)
        };
        let fuzz_h2 = h2(9);
        assert!(
            fuzz_h2 > 0.08,
            "fuzz-face asymmetry must throw a strong 2nd harmonic, got {fuzz_h2:.4}"
        );
        assert!(
            fuzz_h2 > 5.0 * h2(0),
            "fuzz-face must be far more asymmetric than the symmetric ts9"
        );
    }

    #[test]
    fn fuzz_face_has_no_clean_floor() {
        // A fuzz is dirty top to bottom — even with Fuzz all the way down the
        // stage is slammed (it cleans up from the guitar, not this knob).
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(9);
        set_pos(&mut d, 0, 1.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let residual = harmonic_residual(&y, 220.0);
        assert!(
            residual > 0.12,
            "fuzz-face must stay dirty at minimum fuzz, got {residual:.3}"
        );
    }

    #[test]
    fn fuzz_face_cleans_up_at_low_input() {
        // The low-input-impedance trick, heard as level sensitivity: rolled
        // back to a whisper the fuzz rides under its knee and comes out nearly
        // clean, where at full level it is all splat.
        let residual = |scale: f32| -> f64 {
            let mut x = guitar(220.0, SR as usize);
            for s in &mut x {
                *s *= scale;
            }
            let mut d = prepared(9);
            harmonic_residual(&process_in_blocks(&mut d, &x, 256), 220.0)
        };
        let hot = residual(1.0);
        let rolled_back = residual(0.03);
        assert!(hot > 0.2, "full-level fuzz must be all splat, got {hot:.3}");
        assert!(
            rolled_back < 0.3 * hot,
            "rolled-back fuzz must clean up: {rolled_back:.3} vs hot {hot:.3}"
        );
    }

    #[test]
    fn overdrive_is_symmetric_and_suppresses_even_harmonics() {
        // stmlib's SoftClip is an odd function and the tone/makeup stage is
        // linear, so — like the ts9's matched feedback diodes — only odd
        // harmonics survive: no even-harmonic warmth, a clean symmetric
        // overdrive.
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(10);
        set_pos(&mut d, 0, 6.0);
        let y = process_in_blocks(&mut d, &x, 256);
        let h2 = tone_at(&y, 440.0) / tone_at(&y, 220.0);
        assert!(
            h2 < 0.01,
            "overdrive's odd-symmetric clip should suppress even harmonics, got h2/f0 = {h2:.4}"
        );
        // But it is a drive, not a wire — the odd harmonics are there.
        let h3 = tone_at(&y, 660.0) / tone_at(&y, 220.0);
        assert!(h3 > 0.02, "overdrive odd harmonics: h3/f0 = {h3:.4}");
    }

    #[test]
    fn overdrive_drive_knob_takes_it_from_clean_to_dirty() {
        // DaisySP's pre-gain blends a gentle boost into a steep `drive⁵` term,
        // so the lower knob is nearly clean and the upper knob slams the clip.
        let x = guitar(220.0, SR as usize);
        let res = |pos: f32| -> f64 {
            let mut d = prepared(10);
            set_pos(&mut d, 0, pos);
            harmonic_residual(&process_in_blocks(&mut d, &x, 256), 220.0)
        };
        let low = res(3.0);
        let high = res(8.0);
        assert!(
            low < 0.05,
            "low-gain overdrive should stay clean, got {low:.3}"
        );
        assert!(
            high > 0.25,
            "cranked overdrive should clip hard, got {high:.3}"
        );
        assert!(high > 3.0 * low.max(0.001));
    }

    #[test]
    fn overdrive_auto_makeup_holds_level_across_the_sweep() {
        // The technique ported from DaisySP: the post-gain is scheduled as the
        // reciprocal of the clipper's response, so loudness barely moves as the
        // drive climbs. Once past the knee (drive ≥ 5) the driven level sits in
        // a tight band where a plain waveshaper would swing tens of dB — and
        // the DRIVE_CEILING cap keeps the very top of the pot from losing the
        // makeup window and jumping.
        let x = guitar(220.0, SR as usize);
        let in_rms = f64::from(rms(&x[x.len() / 2..]));
        let db = |pos: f32| -> f64 {
            let mut d = prepared(10);
            set_pos(&mut d, 0, pos);
            let y = process_in_blocks(&mut d, &x, 256);
            20.0 * (f64::from(rms(&y[y.len() / 2..])) / in_rms).log10()
        };
        let levels: Vec<f64> = [5.0, 6.0, 7.0, 8.0, 9.0, 10.0]
            .iter()
            .map(|p| db(*p))
            .collect();
        let spread = levels.iter().cloned().fold(f64::MIN, f64::max)
            - levels.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            spread < 3.0,
            "auto-makeup must hold the driven level steady, got {spread:.1} dB spread ({levels:?})"
        );
    }
}
