//! **blues driver** — BD-2-style. Near-full-range gain (lows are kept, the
//! corner sits at 28 Hz), a fixed bright pre-emphasis into the clipper, and
//! *asymmetric* knees (one diode drop against two) for even harmonics; low
//! gain is honestly clean, max is raw breakup. Tone is a ±high shelf.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "bd2",
    name: "Blues Driver",
    params: &PARAMS,
};

/// Asymmetric knees: two diode drops against one. Even harmonics come from
/// the mismatch; the DC it creates is blocked inside the oversampled stage.
const KNEE_POS: f32 = 1.0;
const KNEE_NEG: f32 = 0.5;
/// Fixed bright pre-emphasis into the clipper (+4.6 dB above 1.5 kHz).
const BRIGHT: f32 = 0.7;
/// Calibrated with `ts9_and_blues_driver_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.2;

pub(super) struct BluesDriver {
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
    pub(super) fn new() -> Self {
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
            let x = x + BRIGHT * (x - self.pre_hp.lp(x, self.c1500));
            let v = gain.tick() * x;
            let clipped = if v >= 0.0 {
                KNEE_POS * (v / KNEE_POS).tanh()
            } else {
                KNEE_NEG * (v / KNEE_NEG).tanh()
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
            *s = (x + shelf.tick() * hp) * MAKEUP;
        }
    }
}
