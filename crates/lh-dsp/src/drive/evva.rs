//! **evva** — Lion-Heart's own overdrive: near-full-range gain (30 Hz
//! corner), asymmetric knees (one diode drop against two) for even
//! harmonics, and a 3-band EQ (±12 dB shelves at 120 Hz / 4 kHz, a ±10 dB
//! bandpass at 750 Hz) in place of a single tone knob.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

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
    eq_lo: OnePole,
    /// Mid bandpass: cascaded one-poles for a peak at MID_HZ.
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
    pub(super) fn new() -> Self {
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
        self.c_lo = lp_coeff(LO_HZ, base_rate);
        // Bandpass: wide LP then HP via subtracting a narrower LP.
        self.c_mid_wide = lp_coeff(MID_HZ * 1.4, base_rate);
        self.c_mid_narrow = lp_coeff(MID_HZ / 1.4, base_rate);
        self.c_hi = lp_coeff(HI_HZ, base_rate);
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
