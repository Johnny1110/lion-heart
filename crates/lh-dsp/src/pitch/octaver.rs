//! Octaver: a polyphonic pitch doubler. Three level knobs mix a clean **Dry**
//! path, a **Sub** voice one octave down, and an **Oct** voice one octave up;
//! a **Tone** knob darkens the two shifted voices to tame the granular fizz.
//! POG-flavored (chord-friendly, a little warble) rather than an analog OC-2
//! mono frequency divider — see the family module and ADR 016.

use lh_core::{EffectDesc, ParamDesc};

use super::{Ctl, PedalDef, level_param, tone_param};

static PARAMS: [ParamDesc; 4] = [
    level_param("dry", "Dry", 1.0),
    level_param("sub", "Sub", 0.5),
    level_param("oct", "Oct", 0.0),
    tone_param(0.5),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "octaver",
    name: "Octaver",
    params: &PARAMS,
};

pub const PEDAL: PedalDef = PedalDef {
    desc: &DESC,
    controls: &[Ctl::Dry, Ctl::Sub, Ctl::Oct, Ctl::Tone],
    down_ratio: 0.5, // −1 octave
    up_ratio: 2.0,   // +1 octave
};
