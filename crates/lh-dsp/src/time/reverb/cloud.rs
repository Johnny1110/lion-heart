//! Cloud: the gorgeously big late-'70s ambient wash. The largest tank in
//! the family, dark by default, with the deepest modulation; `haze` fades
//! in the third and fourth diffusion stages, from merely huge to fully
//! smeared.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param, predelay_param,
    tone_param,
};

static PARAMS: [ParamDesc; 6] = [
    decay_param(1.0, 20.0, 8.0),
    predelay_param(250.0, 30.0),
    mix_param(0.5),
    tone_param(800.0, 10_000.0, 3_000.0),
    mod_param(0.4),
    knob_param("haze", "Haze", 0.6),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "cloud",
    name: "Cloud",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Decay,
        Ctl::Predelay,
        Ctl::Mix,
        Ctl::Tone,
        Ctl::Mod,
        Ctl::Haze,
    ],
    kind: Kind::Tank,
    insert: Insert::None,
    scale_min: 1.8,
    scale_max: 1.8,
    diff_count: 4,
    diff_g: 0.68,
    lfo_hz: 0.5,
    mod_max_ms: 5.0,
    swell: false,
    bloom: false,
    wet_gain: 0.95,
};
