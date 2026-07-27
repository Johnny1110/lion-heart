//! **evva** — Lion-Heart's own overdrive: near-full-range gain (30 Hz
//! corner), asymmetric knees (one diode drop against two) for even
//! harmonics, and a 3-band EQ (±12 dB shelves at 120 Hz / 4 kHz, a ±10 dB
//! bandpass at 750 Hz) in place of a single tone knob.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use crate::blocks::waveshaper::{Adaa1, asym_tanh, asym_tanh_f1};
use crate::eq::tonestack::kind;

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

/// Anti-aliased clipping (PRD 024). A `tanh` knee aliases far less than a hard
/// corner, but at high gain it approaches one — first-order ADAA, since
/// `tanh`'s second antiderivative is not elementary.
pub(super) struct Evva {
    clip: Adaa1,
    hp30: OnePole,
    dc_os: OnePole,
    stack: ToneStack,
    c30: f32,
    c12: f32,
}

impl Evva {
    pub(super) fn new() -> Self {
        Self {
            clip: Adaa1::new(),
            hp30: OnePole::default(),
            dc_os: OnePole::default(),
            stack: ToneStack::new(kind::BASSMAN),
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
        self.clip.reset();
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
            let clipped = self.clip.process(
                v,
                |u| asym_tanh(u, KNEE_POS as f64, KNEE_NEG as f64),
                |u| asym_tanh_f1(u, KNEE_POS as f64, KNEE_NEG as f64),
            );
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
        // A real Bassman tone stack: the one Fender voice in the family.
        self.stack.process(block, low, mid, high);
    }
}
