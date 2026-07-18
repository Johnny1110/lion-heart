//! **angry-charlie** — JHS Angry Charlie V3-style "Marshall in a box". The
//! circuit's whole personality is *where* the clipping happens: two clean
//! op-amp gain stages (a swept inverting preamp, then a fixed buffer) feed a
//! pair of red LEDs wired symmetrically **to ground, outside the op-amp's
//! feedback loop** — unlike the ts9's diodes-in-the-loop soft compression,
//! there is no gain reduction as the signal approaches the knee. The signal
//! rides clean until the LEDs' (higher than silicon) forward voltage is
//! reached, then slams flat — a genuine hard clip, not a tanh curve, and
//! symmetric enough that even harmonics mostly cancel, leaving a buzzsaw
//! stack of odds. A fixed bright pre-emphasis feeds that edge (the sizzle
//! this pedal is known for); unlike the red-charlie/monster5150, the lows
//! are left full and un-carved ahead of the clip — the big, thick low end
//! reviewers describe. A Marshall-style Baxandall Bass/Middle/Treble stack
//! (post-clip, like the amp's own tone stack) and Volume finish it off.

use lh_core::{EffectDesc, ParamDesc, db_to_lin};

use super::{Circuit, OnePole, Ramp, ToneStack, knob, lp_coeff};

static PARAMS: [ParamDesc; 5] = [
    knob("gain", "Gain", 5.0, 20.0),
    knob("bass", "Bass", 5.0, 30.0),
    knob("middle", "Middle", 5.0, 30.0),
    knob("treble", "Treble", 5.0, 30.0),
    knob("volume", "Volume", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "angry-charlie",
    name: "Angry Charlie",
    params: &PARAMS,
};

/// Symmetric hard-clip threshold: the red LEDs' forward voltage, mapped into
/// the same normalized headroom the rest of the family uses. Both polarities
/// share it — that symmetry is what starves the even harmonics.
const KNEE: f32 = 1.0;
/// Fixed bright pre-emphasis ahead of the clipper (+~3 dB above 1.8 kHz):
/// the sizzle that's there at every gain setting, same trick as the
/// monster5150's pre-emphasis but voiced brighter.
const BRIGHT: f32 = 0.35;
/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.09;

/// Marshall-style Baxandall corners.
const BASS_HZ: f32 = 90.0;
const MID_HZ: f32 = 500.0;
const TREBLE_HZ: f32 = 2_800.0;

pub(super) struct AngryCharlie {
    hp_in: OnePole,
    bright: OnePole,
    dc_os: OnePole,
    stack: ToneStack,
    c25: f32,
    c1800: f32,
    c12: f32,
}

impl AngryCharlie {
    pub(super) fn new() -> Self {
        Self {
            hp_in: OnePole::default(),
            bright: OnePole::default(),
            dc_os: OnePole::default(),
            stack: ToneStack::new(BASS_HZ, MID_HZ, TREBLE_HZ),
            c25: 0.0,
            c1800: 0.0,
            c12: 0.0,
        }
    }
}

impl Circuit for AngryCharlie {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.c25 = lp_coeff(25.0, os_rate);
        self.c1800 = lp_coeff(1_800.0, os_rate);
        self.c12 = lp_coeff(12.0, os_rate);
        self.stack.prepare(base_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.hp_in.reset();
        self.bright.reset();
        self.dc_os.reset();
        self.stack.reset();
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        // Both op-amp stages are clean gain, not clippers — +14 dB (edge of
        // breakup against the LEDs) to +60 dB (slamming the wall). Audio
        // taper, powf twice per chunk, ramped per sample.
        let mut gain = Ramp::over(drive, |d| db_to_lin(14.0 + 46.0 * (d * 0.1).powf(1.5)));
        for s in block.iter_mut() {
            let x = *s;
            // Subsonic block at 25 Hz — the lows otherwise stay full.
            let x = x - self.hp_in.lp(x, self.c25);
            // Fixed bright pre-emphasis into the clipper.
            let x = x + BRIGHT * (x - self.bright.lp(x, self.c1800));
            // The two clean gain stages, then a genuine hard clip to ground
            // — flat-topped, not a tanh curve, symmetric both polarities.
            let clipped = (gain.tick() * x).clamp(-KNEE, KNEE);
            *s = clipped - self.dc_os.lp(clipped, self.c12);
        }
    }

    fn post(&mut self, block: &mut [f32], _tone: &[f32]) {
        // No single tone knob — tone shaping lives in `eq`; `post` only
        // applies the output makeup.
        for s in block.iter_mut() {
            *s *= MAKEUP;
        }
    }

    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]) {
        // Shared 3-band Baxandall stack, voiced at 90 Hz / 500 Hz / 2.8 kHz.
        self.stack.process(block, low, mid, high);
    }
}
