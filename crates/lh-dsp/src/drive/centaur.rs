//! **centaur** — Klon-style. A clean path (never clips — the 18 V charge
//! pump's headroom) is always in the mix; the gain knob blends in a
//! 250 Hz-high-passed path squashed by germanium diodes (soft ~0.35 V
//! knees). Low gain is the famous transparent boost; the knobs follow the
//! original face: Gain / Treble / Output.
//!
//! The clean path carries a gentle low-shelf cut (~−6 dB below 130 Hz): the
//! real Klon is a mid-forward boost that *tightens* the low end, not a flat
//! full-range lift. This is what keeps it usable stacked in front of a
//! high-gain pedal — a flat boost pours bass into the next stage's clipper
//! and the lows fart out (see `drive_stacking_stays_tight`).

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("output", "Output", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "centaur",
    name: "Centaur",
    params: &PARAMS,
};

/// Germanium knee — much lower and softer than silicon.
const KNEE: f32 = 0.35;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs` and
/// `centaur_low_gain_is_a_transparent_boost`.
const MAKEUP: f32 = 0.65;
/// Clean-path low-shelf depth: `x + LOW_TILT·lp` cuts the sub-130 Hz band by
/// ~6 dB (LOW_TILT −0.5 → half the low content), tightening the boost.
const LOW_TILT: f32 = -0.5;

pub(super) struct Centaur {
    low_lp: OnePole,
    hp250: OnePole,
    treble_hp: OnePole,
    dc: OnePole,
    c_low: f32,
    c250: f32,
    c1200: f32,
    c_dc: f32,
}

impl Centaur {
    pub(super) fn new() -> Self {
        Self {
            low_lp: OnePole::default(),
            hp250: OnePole::default(),
            treble_hp: OnePole::default(),
            dc: OnePole::default(),
            c_low: 0.0,
            c250: 0.0,
            c1200: 0.0,
            c_dc: 0.0,
        }
    }

    /// Germanium pair: even softer knee than silicon — `u/(1+|u|)` instead
    /// of `tanh`, scaled to the diode drop.
    #[inline]
    fn germanium(v: f32) -> f32 {
        let u = v / KNEE;
        KNEE * (u / (1.0 + u.abs()))
    }
}

impl Circuit for Centaur {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c_low = lp_coeff(130.0, os_rate);
        self.c250 = lp_coeff(250.0, os_rate);
        self.c1200 = lp_coeff(1_200.0, base_rate);
        self.c_dc = lp_coeff(10.0, base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.low_lp.reset();
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
            // Clean path, low-shelf-tightened: the mid-forward Klon boost.
            let clean = x + LOW_TILT * self.low_lp.lp(x, self.c_low);
            let dirty_in = x - self.hp250.lp(x, self.c250);
            let dirty = Self::germanium(gain.tick() * dirty_in);
            *s = (1.2 + 0.8 * b) * clean + b * b * dirty;
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        // Treble: a gentle ±6 dB shelf above 1.2 kHz, flat at noon.
        let mut shelf = Ramp::over(tone, |t| db_to_lin(-6.0 + 1.2 * t) - 1.0);
        for s in block.iter_mut() {
            let x = *s;
            let hp = x - self.treble_hp.lp(x, self.c1200);
            let y = (x + shelf.tick() * hp) * MAKEUP;
            *s = y - self.dc.lp(y, self.c_dc);
        }
    }
}
