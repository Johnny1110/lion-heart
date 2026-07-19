//! Reflections: the room without the tail — a psycho-acoustic early-
//! reflection pattern, no feedback anywhere. `shape` picks the tap table
//! (studio / chamber / dome), `size` stretches its window; use it to move
//! the amp off the grille cloth and into a space.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, REFLECTION_SHAPES, VoiceDef, knob_param, mix_param, predelay_param,
    stepped_param, tone_param,
};

static PARAMS: [ParamDesc; 5] = [
    predelay_param(80.0, 0.0),
    mix_param(0.35),
    tone_param(2_000.0, 14_000.0, 8_000.0),
    knob_param("size", "Size", 0.4),
    stepped_param("shape", "Shape", REFLECTION_SHAPES, 0.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "reflections",
    name: "Reflections",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[Ctl::Predelay, Ctl::Mix, Ctl::Tone, Ctl::Size, Ctl::Shape],
    kind: Kind::Early,
    insert: Insert::None,
    // For `Early`, scale_min/max are the reflection window in ms at
    // size 0 and 1 (not a tank line scale).
    scale_min: 15.0,
    scale_max: 120.0,
    diff_count: 1,
    diff_g: 0.35,
    lfo_hz: 0.0,
    mod_max_ms: 0.0,
    swell: false,
    bloom: false,
    wet_gain: 0.9,
};
