//! **classic** — the original Lion-Heart biased-tanh waveshaper, kept so
//! v1 presets keep their exact sound (the preset migration targets it —
//! `lh_core::preset::CLASSIC_DRIVE_MODEL`).

use lh_core::{EffectDesc, ParamDesc, db_to_lin, drive_law};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static PARAMS: [ParamDesc; 3] = [
    knob("drive", "Drive", 5.0, 20.0),
    knob("tone", "Tone", 5.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "classic",
    name: "Classic",
    params: &PARAMS,
};

/// Static bias into the tanh: breaks symmetry so even harmonics appear
/// (tube-flavoured). The constant output offset is removed analytically and
/// the signal-dependent remainder by the DC blocker.
const BIAS: f32 = 0.2;
/// tanh(BIAS), precomputed (rustc has no const tanh).
const BIAS_TANH: f32 = 0.197_375_32;
const DC_R: f32 = 0.995;

pub(super) struct Classic {
    dc_x1: f32,
    dc_y1: f32,
    lp: OnePole,
    tone_c: f32,
    base_rate: f32,
}

impl Classic {
    pub(super) fn new() -> Self {
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
        self.tone_c = lp_coeff(drive_law::classic_tone_hz(PARAMS[1].default), base_rate);
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
