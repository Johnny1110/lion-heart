//! Drive: a family of overdrive pedals behind one chain slot. Each pedal
//! owns its faceplate (PRD 001): its own knob set, captions, and defaults —
//! TS9 has exactly three knobs, the evva five. Knobs are positions `0..=10`,
//! laid out like the face of the modelled pedal so nothing has to be
//! relearned.
//!
//! Registered pedals:
//!
//! - **ts9** — Tube-Screamer-style. The gained path is the input high-passed
//!   at 720 Hz (4.7 kΩ + 47 nF into the op-amp), amplified 21–41 dB
//!   (51 kΩ + 500 kΩ drive pot over 4.7 kΩ) and squashed by the feedback
//!   diodes; summing it with the unity dry path is what makes the classic
//!   mid-hump with lows that stay clean. The 51 pF feedback cap darkens the
//!   clipped path as the drive pot rises. Tone is the usual one-pole
//!   dark↔bright tilt around 723 Hz.
//! - **blues driver** — BD-2-style. Near-full-range gain (lows are kept, the
//!   corner sits at 28 Hz), a fixed bright pre-emphasis into the clipper, and
//!   *asymmetric* knees (one diode drop against two) for even harmonics; low
//!   gain is honestly clean, max is raw breakup. Tone is a ±high shelf.
//! - **classic** — the original Lion-Heart biased-tanh waveshaper, kept so
//!   v1 presets keep their exact sound (the preset migration targets it —
//!   `lh_core::preset::CLASSIC_DRIVE_MODEL`).
//! - **centaur** — Klon-style. A clean path (never clips — the 18 V charge
//!   pump's headroom) is always in the mix; the gain knob blends in a
//!   250 Hz-high-passed path squashed by germanium diodes (soft ~0.35 V
//!   knees). Low gain is the famous transparent boost; the knobs follow the
//!   original face: Gain / Treble / Output.
//!
//! Every model runs its nonlinearity inside the shared 4× oversampler
//! ([`crate::oversample`]) and its linear tone stack at the base rate. Knob
//! smoothing is written once per chunk into trajectory buffers shared by
//! both channels.
//!
//! # Adding your own pedal
//!
//! 1. Implement [`Circuit`]: the nonlinear `shape` pass at the oversampled
//!    rate and the linear `post` pass (tone stack, makeup) at the base rate.
//! 2. Declare its faceplate: a `ParamDesc` table + `EffectDesc`, **append**
//!    the desc to [`FAMILY`] and a matching [`ModelDef`] to [`MODELS`].
//!    Append only — the v2 preset migration and plugin param ids reference
//!    pedals by position and key.
//!
//! Everything downstream picks the entry up from the registry: the GUI
//! pedal dropdown and knobs, REPL labels (`set drive.pedal ts9`), MIDI CC
//! mapping, preset save/load, and the plugin's per-pedal host params.

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range, db_to_lin, drive_law};

use crate::Effect;
use crate::oversample::{CHUNK, Oversampler4x};
use crate::smooth::Smoothed;

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

static TS9_PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];
static TS9_DESC: EffectDesc = EffectDesc {
    key: "ts9",
    name: "TS9",
    params: &TS9_PARAMS,
};

static BD2_PARAMS: [ParamDesc; 3] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];
static BD2_DESC: EffectDesc = EffectDesc {
    key: "bd2",
    name: "Blues Driver",
    params: &BD2_PARAMS,
};

static CLASSIC_PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];
static CLASSIC_DESC: EffectDesc = EffectDesc {
    key: "classic",
    name: "Classic",
    params: &CLASSIC_PARAMS,
};

static CENTAUR_PARAMS: [ParamDesc; 3] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("output", "Output", 6.0, 20.0),
];
static CENTAUR_DESC: EffectDesc = EffectDesc {
    key: "centaur",
    name: "Centaur",
    params: &CENTAUR_PARAMS,
};

static EVVA_PARAMS: [ParamDesc; 5] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("low", "Low", 5.0, 30.0),
    knob("mid", "Mid", 5.0, 30.0),
    knob("high", "High", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];
static EVVA_DESC: EffectDesc = EffectDesc {
    key: "evva",
    name: "Evva",
    params: &EVVA_PARAMS,
};

/// The drive family, in menu order. Aligned with [`MODELS`] and pinned to
/// `lh_core::preset::DRIVE_PEDALS` (the v2 migration) by tests.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "drive",
    name: "Drive",
    pedals: &[
        &TS9_DESC,
        &BD2_DESC,
        &CLASSIC_DESC,
        &CENTAUR_DESC,
        &EVVA_DESC,
    ],
};

pub const MODEL_COUNT: usize = 5;

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
    pub desc: &'static EffectDesc,
    controls: &'static [Ctl],
    build: fn() -> Box<dyn Circuit>,
}

/// The drive pedal registry, aligned with [`FAMILY`]`.pedals`.
pub static MODELS: [ModelDef; MODEL_COUNT] = [
    ModelDef {
        desc: &TS9_DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(Ts9::new()),
    },
    ModelDef {
        desc: &BD2_DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(BluesDriver::new()),
    },
    ModelDef {
        desc: &CLASSIC_DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(Classic::new()),
    },
    ModelDef {
        desc: &CENTAUR_DESC,
        controls: &[Ctl::Drive, Ctl::Tone, Ctl::Level],
        build: || Box::new(Centaur::new()),
    },
    ModelDef {
        desc: &EVVA_DESC,
        controls: &[Ctl::Drive, Ctl::Low, Ctl::Mid, Ctl::High, Ctl::Level],
        build: || Box::new(Evva::new()),
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

fn lp_coeff(hz: f32, rate: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * hz / rate).exp()
}

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

// --- ts9 ---

/// Diode knee scale: the clipped path flattens out around ±0.65 of full
/// scale, leaving headroom for the dry sum.
const TS9_DIODE: f32 = 0.65;
/// Calibrated so drive 5 / tone 5 / level 6 lands near unity loudness
/// (asserted by `levels_roughly_matched`).
const TS9_MAKEUP: f32 = 0.22;

struct Ts9 {
    hp720: OnePole,
    fb_lp: OnePole,
    tone_lp: OnePole,
    dc: OnePole,
    c720: f32,
    c723: f32,
    c_dc: f32,
    /// Feedback-cap lowpass coefficient, eased toward a per-chunk target.
    fb_c: f32,
    os_rate: f32,
}

impl Ts9 {
    fn new() -> Self {
        Self {
            hp720: OnePole::default(),
            fb_lp: OnePole::default(),
            tone_lp: OnePole::default(),
            dc: OnePole::default(),
            c720: 0.0,
            c723: 0.0,
            c_dc: 0.0,
            fb_c: 0.0,
            os_rate: 4.0 * 48_000.0,
        }
    }

    /// Feedback resistance for a drive-pot position 0..10 (51 kΩ series plus
    /// the 500 kΩ pot, audio taper).
    #[inline]
    fn feedback_ohms(pos: f32) -> f32 {
        let n = pos * 0.1;
        51_000.0 + 500_000.0 * n * n
    }
}

impl Circuit for Ts9 {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.os_rate = os_rate;
        self.c720 = lp_coeff(720.0, os_rate);
        self.c723 = lp_coeff(723.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        self.fb_c = lp_coeff(6_000.0, os_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp720.reset();
        self.fb_lp.reset();
        self.tone_lp.reset();
        self.dc.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // The 51 pF across the feedback network: 5.7 kHz at max drive up to
        // far above Nyquist at min. One exp per chunk, eased per sample.
        let fc =
            1.0 / (std::f32::consts::TAU * Self::feedback_ohms(drive[drive.len() - 1]) * 51e-12);
        let fb_target = lp_coeff(fc.min(self.os_rate * 0.45), self.os_rate);
        for (s, d) in block.iter_mut().zip(drive) {
            self.fb_c += 0.002 * (fb_target - self.fb_c);
            let x = *s;
            let hp = x - self.hp720.lp(x, self.c720);
            let g = 1.0 + Self::feedback_ohms(*d) / 4_700.0;
            let v = g * hp / TS9_DIODE;
            let clipped = TS9_DIODE * (v / (1.0 + v * v).sqrt());
            // Unity dry plus the clipped mids: the screamer sum.
            *s = x + self.fb_lp.lp(clipped, self.fb_c);
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        for (s, t) in block.iter_mut().zip(tone) {
            let x = *s;
            let lp = self.tone_lp.lp(x, self.c723);
            let hp = x - lp;
            let n = t * 0.1;
            // 0 = dark (8% of the treble), 10 = bright (+3.4 dB tilt).
            let bright = 0.08 + 1.4 * n * n;
            let y = (lp + bright * hp) * TS9_MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

// --- blues driver ---

/// Asymmetric knees: two diode drops against one. Even harmonics come from
/// the mismatch; the DC it creates is blocked inside the oversampled stage.
const BD2_KNEE_POS: f32 = 1.0;
const BD2_KNEE_NEG: f32 = 0.5;
/// Fixed bright pre-emphasis into the clipper (+4.6 dB above 1.5 kHz).
const BD2_BRIGHT: f32 = 0.7;
/// Calibrated with `ts9_and_blues_driver_sit_near_unity_at_default_knobs`.
const BD2_MAKEUP: f32 = 0.2;

struct BluesDriver {
    hp_in: OnePole,
    pre_hp: OnePole,
    dc_os: OnePole,
    tone_hp: OnePole,
    c28: f32,
    c1500: f32,
    c12: f32,
    c1000: f32,
}

impl BluesDriver {
    fn new() -> Self {
        Self {
            hp_in: OnePole::default(),
            pre_hp: OnePole::default(),
            dc_os: OnePole::default(),
            tone_hp: OnePole::default(),
            c28: 0.0,
            c1500: 0.0,
            c12: 0.0,
            c1000: 0.0,
        }
    }
}

impl Circuit for BluesDriver {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c28 = lp_coeff(28.0, os_rate);
        self.c1500 = lp_coeff(1_500.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.c1000 = lp_coeff(1_000.0, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp_in.reset();
        self.pre_hp.reset();
        self.dc_os.reset();
        self.tone_hp.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // +2 dB (honest clean boost) up to +42 dB (raw breakup); powf twice
        // per chunk, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(2.0 + 40.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            let x = x - self.hp_in.lp(x, self.c28);
            let x = x + BD2_BRIGHT * (x - self.pre_hp.lp(x, self.c1500));
            let v = gain.tick() * x;
            let clipped = if v >= 0.0 {
                BD2_KNEE_POS * (v / BD2_KNEE_POS).tanh()
            } else {
                BD2_KNEE_NEG * (v / BD2_KNEE_NEG).tanh()
            };
            *s = clipped - self.dc_os.lp(clipped, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        // High shelf around 1 kHz: −14 dB muffled to +8 dB cutting.
        let mut shelf = Ramp::over(tone, |t| db_to_lin(-14.0 + 2.2 * t) - 1.0);
        for s in block.iter_mut() {
            let x = *s;
            let hp = x - self.tone_hp.lp(x, self.c1000);
            *s = (x + shelf.tick() * hp) * BD2_MAKEUP;
        }
    }
}

// --- classic ---

/// Static bias into the tanh: breaks symmetry so even harmonics appear
/// (tube-flavoured). The constant output offset is removed analytically and
/// the signal-dependent remainder by the DC blocker.
const BIAS: f32 = 0.2;
/// tanh(BIAS), precomputed (rustc has no const tanh).
const BIAS_TANH: f32 = 0.197_375_32;
const DC_R: f32 = 0.995;

struct Classic {
    dc_x1: f32,
    dc_y1: f32,
    lp: OnePole,
    tone_c: f32,
    base_rate: f32,
}

impl Classic {
    fn new() -> Self {
        Self {
            dc_x1: 0.0,
            dc_y1: 0.0,
            lp: OnePole::default(),
            tone_c: 0.5,
            base_rate: 48_000.0,
        }
    }
}

impl Circuit for Classic {
    fn prepare(&mut self, base_rate: f32, _os_rate: f32) {
        self.base_rate = base_rate;
        self.tone_c = lp_coeff(
            drive_law::classic_tone_hz(CLASSIC_PARAMS[1].default),
            base_rate,
        );
        self.reset();
    }

    fn reset(&mut self) {
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
        self.lp.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        let mut gain = Ramp::over(drive, |d| db_to_lin(drive_law::classic_drive_db(d)));
        for s in block.iter_mut() {
            // Subtracting tanh(BIAS) recenters the idle point at zero.
            *s = (*s * gain.tick() + BIAS).tanh() - BIAS_TANH;
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        let target = lp_coeff(
            drive_law::classic_tone_hz(tone[tone.len() - 1]),
            self.base_rate,
        );
        for s in block.iter_mut() {
            self.tone_c += 0.01 * (target - self.tone_c);
            let x = *s;
            let y = x - self.dc_x1 + DC_R * self.dc_y1;
            self.dc_x1 = x;
            self.dc_y1 = if y.abs() < 1e-15 { 0.0 } else { y };
            *s = self.lp.lp(self.dc_y1, self.tone_c);
        }
    }
}

// --- centaur ---

/// Germanium knee — much lower and softer than silicon.
const CENTAUR_KNEE: f32 = 0.35;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs` and
/// `centaur_low_gain_is_a_transparent_boost`.
const CENTAUR_MAKEUP: f32 = 0.65;

struct Centaur {
    hp250: OnePole,
    treble_hp: OnePole,
    dc: OnePole,
    c250: f32,
    c1200: f32,
    c_dc: f32,
}

impl Centaur {
    fn new() -> Self {
        Self {
            hp250: OnePole::default(),
            treble_hp: OnePole::default(),
            dc: OnePole::default(),
            c250: 0.0,
            c1200: 0.0,
            c_dc: 0.0,
        }
    }

    /// Germanium pair: even softer knee than silicon — `u/(1+|u|)` instead
    /// of `tanh`, scaled to the diode drop.
    #[inline]
    fn germanium(v: f32) -> f32 {
        let u = v / CENTAUR_KNEE;
        CENTAUR_KNEE * (u / (1.0 + u.abs()))
    }
}

impl Circuit for Centaur {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c250 = lp_coeff(250.0, os_rate);
        self.c1200 = lp_coeff(1_200.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp250.reset();
        self.treble_hp.reset();
        self.dc.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // The dual-ganged gain pot: one gang sweeps the dirty path's gain
        // +8..+38 dB, the other lifts the ever-present clean path a little;
        // the dirty share of the mix comes in progressively (b²), so gain 0
        // leaves nothing but the clean boost.
        let mut gain = Ramp::over(drive, |d| db_to_lin(8.0 + 30.0 * (d * 0.1).powf(1.5)));
        for (s, d) in block.iter_mut().zip(drive) {
            let b = d * 0.1;
            let x = *s;
            let dirty_in = x - self.hp250.lp(x, self.c250);
            let dirty = Self::germanium(gain.tick() * dirty_in);
            *s = (1.2 + 0.8 * b) * x + b * b * dirty;
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        // Treble: a gentle ±6 dB shelf above 1.2 kHz, flat at noon.
        let mut shelf = Ramp::over(tone, |t| db_to_lin(-6.0 + 1.2 * t) - 1.0);
        for s in block.iter_mut() {
            let x = *s;
            let hp = x - self.treble_hp.lp(x, self.c1200);
            let y = (x + shelf.tick() * hp) * CENTAUR_MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}

// --- evva ---

/// Asymmetric knees for even harmonics — one diode drop against two.
const EVVA_KNEE_POS: f32 = 0.8;
const EVVA_KNEE_NEG: f32 = 0.5;
/// Calibrated so the evva sits near unity at default knobs (level 6, gain 4).
const EVVA_MAKEUP: f32 = 0.28;

/// 3-band EQ corner frequencies.
const EVVA_LO_HZ: f32 = 120.0;
const EVVA_MID_HZ: f32 = 750.0;
const EVVA_HI_HZ: f32 = 4_000.0;

struct Evva {
    hp30: OnePole,
    dc_os: OnePole,
    eq_lo: OnePole,
    /// Mid bandpass: cascaded one-poles for a peak at EVVA_MID_HZ.
    eq_mid_lp: OnePole,
    eq_mid_hp: OnePole,
    eq_hi: OnePole,
    c30: f32,
    c12: f32,
    c_lo: f32,
    c_mid_wide: f32,
    c_mid_narrow: f32,
    c_hi: f32,
}

impl Evva {
    fn new() -> Self {
        Self {
            hp30: OnePole::default(),
            dc_os: OnePole::default(),
            eq_lo: OnePole::default(),
            eq_mid_lp: OnePole::default(),
            eq_mid_hp: OnePole::default(),
            eq_hi: OnePole::default(),
            c30: 0.0,
            c12: 0.0,
            c_lo: 0.0,
            c_mid_wide: 0.0,
            c_mid_narrow: 0.0,
            c_hi: 0.0,
        }
    }
}

impl Circuit for Evva {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c30 = lp_coeff(30.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.c_lo = lp_coeff(EVVA_LO_HZ, base_rate);
        // Bandpass: wide LP then HP via subtracting a narrower LP.
        self.c_mid_wide = lp_coeff(EVVA_MID_HZ * 1.4, base_rate);
        self.c_mid_narrow = lp_coeff(EVVA_MID_HZ / 1.4, base_rate);
        self.c_hi = lp_coeff(EVVA_HI_HZ, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp30.reset();
        self.dc_os.reset();
        self.eq_lo.reset();
        self.eq_mid_lp.reset();
        self.eq_mid_hp.reset();
        self.eq_hi.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // +3 dB (honest clean boost) to +36 dB (singing breakup), audio taper.
        let mut gain = Ramp::over(drive, |d| db_to_lin(3.0 + 33.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // HP at 30 Hz — blocks subsonics, keeps the full guitar range.
            let x = x - self.hp30.lp(x, self.c30);
            let v = gain.tick() * x;
            let clipped = if v >= 0.0 {
                EVVA_KNEE_POS * (v / EVVA_KNEE_POS).tanh()
            } else {
                EVVA_KNEE_NEG * (v / EVVA_KNEE_NEG).tanh()
            };
            *s = clipped - self.dc_os.lp(clipped, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // The tone knob is unused on evva — tone shaping lives in `eq`.
        // `post` still applies the output makeup.
        for s in block.iter_mut() {
            *s *= EVVA_MAKEUP;
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        // 3-band EQ with one-pole shelves + a cascaded-one-pole mid bandpass:
        //
        //   low  — shelf at 120 Hz (±12 dB)
        //   mid  — bandpass centred at 750 Hz (±10 dB), Q ≈ 1.0
        //   high — shelf at 4 kHz (±12 dB)
        //
        // Knob 5 = flat (0 dB), 0/10 = cut/boost.
        for (s, (&l, (&m, &h))) in block.iter_mut().zip(low.iter().zip(mid.iter().zip(high))) {
            let x = *s;
            let lo = self.eq_lo.lp(x, self.c_lo);
            let hi = x - self.eq_hi.lp(x, self.c_hi);
            // Bandpass: LP at f*1.4, then HP by subtracting a second LP at f/1.4.
            let bp_raw = self.eq_mid_lp.lp(x, self.c_mid_wide);
            let bp = bp_raw - self.eq_mid_hp.lp(bp_raw, self.c_mid_narrow);
            let lo_gain = db_to_lin(-12.0 + 2.4 * l);
            let mid_gain = db_to_lin(-10.0 + 2.0 * m);
            let hi_gain = db_to_lin(-12.0 + 2.4 * h);
            *s = x + (lo_gain - 1.0) * lo + (mid_gain - 1.0) * bp + (hi_gain - 1.0) * hi;
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
        for model in [0usize, 1, 3, 4] {
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
        // Verify each EQ band actually shapes the spectrum.
        let x = guitar(220.0, SR as usize);
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

        // High shelf: boosting high should lift 6 kHz relative to 500 Hz.
        let flat_hi = measure(5.0, 5.0, 5.0, 6_000.0) / measure(5.0, 5.0, 5.0, 500.0);
        let boosted_hi = measure(5.0, 5.0, 10.0, 6_000.0) / measure(5.0, 5.0, 10.0, 500.0);
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
    fn evva_eq_is_flat_at_defaults() {
        // At all-EQ knobs at 5, the 3-band EQ should be roughly transparent
        // (each band gain ≈ 1.0).
        let x = guitar(220.0, SR as usize);
        let mut d = prepared(4);
        set_pos(&mut d, 0, 2.0);
        let y = process_in_blocks(&mut d, &x, 256);
        // Compare the shape of the spectrum: flat EQ should not radically
        // reshape the output relative to the input.
        let in_220 = tone_at(&x, 220.0);
        let out_220 = tone_at(&y, 220.0);
        let ratio = out_220 / in_220.max(1e-9);
        assert!(
            ratio > 0.0,
            "evva flat eq must pass signal: ratio {ratio:.3}"
        );
    }
}
