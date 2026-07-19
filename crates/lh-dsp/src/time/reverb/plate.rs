//! Plate: rich, fast-building, bright — depth without early-reflection cues
//! to a real space. All four diffusers at a high coefficient give the
//! instant density plates are loved for; the tone range reaches higher than
//! any other voice.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param, predelay_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(0.3, 6.0, 2.2),
    predelay_param(120.0, 5.0),
    mix_param(0.3),
    tone_param(2_000.0, 16_000.0, 9_000.0),
    mod_param(0.1),
    knob_param("size", "Size", 0.5),
    knob_param("lowend", "Low End", 0.5),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "plate",
    name: "Plate",
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
        Ctl::Size,
        Ctl::LowEnd,
    ],
    kind: Kind::Tank,
    insert: Insert::None,
    scale_min: 0.5,
    scale_max: 1.2,
    diff_count: 4,
    diff_g: 0.72,
    lfo_hz: 0.6,
    mod_max_ms: 1.2,
    swell: false,
    bloom: false,
    wet_gain: 1.0,
};
