//! **evva** — Lion-Heart's own overdrive: near-full-range gain (30 Hz
//! corner), asymmetric knees (one diode drop against two) for even
//! harmonics, and a 3-band EQ (±12 dB shelves at 120 Hz / 4 kHz, a ±10 dB
//! bandpass at 750 Hz) in place of a single tone knob.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, ToneStack, knob, lp_coeff};

static PARAMS: [ParamDesc; 5] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("low", "Low", 5.0, 30.0),
    knob("mid", "Mid", 5.0, 30.0),
    knob("high", "High", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "evva",
    name: "Evva",
    params: &PARAMS,
};

/// Asymmetric knees for even harmonics — one diode drop against two.
const KNEE_POS: f32 = 0.8;
const KNEE_NEG: f32 = 0.5;
/// Calibrated so the evva sits near unity at default knobs (level 6, gain 4).
const MAKEUP: f32 = 0.28;

/// 3-band EQ corner frequencies.
const LO_HZ: f32 = 120.0;
const MID_HZ: f32 = 750.0;
const HI_HZ: f32 = 4_000.0;

pub(super) struct Evva {
    hp30: OnePole,
    dc_os: OnePole,
    stack: ToneStack,
    c30: f32,
    c12: f32,
}

impl Evva {
    pub(super) fn new() -> Self {
        Self {
            hp30: OnePole::default(),
            dc_os: OnePole::default(),
            stack: ToneStack::new(LO_HZ, MID_HZ, HI_HZ),
            c30: 0.0,
            c12: 0.0,
        }
    }
}

impl Circuit for Evva {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c30 = lp_coeff(30.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.stack.prepare(base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp30.reset();
        self.dc_os.reset();
        self.stack.reset();
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
                KNEE_POS * (v / KNEE_POS).tanh()
            } else {
                KNEE_NEG * (v / KNEE_NEG).tanh()
            };
            *s = clipped - self.dc_os.lp(clipped, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // The tone knob is unused on evva — tone shaping lives in `eq`.
        // `post` still applies the output makeup.
        for s in block.iter_mut() {
            *s *= MAKEUP;
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        // Shared 3-band stack, voiced at 120 Hz / 750 Hz / 4 kHz.
        self.stack.process(block, low, mid, high);
    }
}
