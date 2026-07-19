//! Hall: diffused reflections, slow-building density — the M5 FDN voicing
//! and therefore the v4→v5 migration target. At defaults (size noon = scale
//! 1.0, mod 0, low end neutral) this *is* the old `reverb` pedal:
//! `decay`/`tone`/`predelay`/`mix` keep their v4 keys, ranges, and defaults,
//! so migrated files sound the same.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param, predelay_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(0.2, 8.0, 1.8),
    predelay_param(120.0, 20.0),
    mix_param(0.3),
    tone_param(1_000.0, 12_000.0, 5_000.0),
    mod_param(0.0), // purist default: the M5 tail had no wobble
    knob_param("size", "Size", 0.5),
    knob_param("lowend", "Low End", 0.5),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "hall",
    name: "Hall",
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
    // Geometric size sweep chosen so noon is exactly scale 1.0 (0.6 ×
    // 1.6667 = 1.0): club at 7 o'clock, arena at 5.
    scale_min: 0.6,
    scale_max: 1.666_67,
    diff_count: 2,
    diff_g: 0.7,
    lfo_hz: 0.35,
    mod_max_ms: 2.5,
    swell: false,
    bloom: false,
    wet_gain: 1.0,
};
