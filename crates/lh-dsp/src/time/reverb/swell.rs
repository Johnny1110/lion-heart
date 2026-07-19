//! Swell: a volume pedal on the wet — every detected note onset restarts a
//! `rise`-long ramp into the reverb, for evolving textures behind the dry.
//! `mode` picks what fades in: the reverb alone, or dry + reverb for full
//! bowed-pad ambience.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, mix_param, mod_param, predelay_param, stepped_param,
    tone_param,
};

pub const SWELL_MODES: &[&str] = &["reverb", "dry+reverb"];

static PARAMS: [ParamDesc; 7] = [
    decay_param(0.5, 10.0, 3.5),
    predelay_param(250.0, 20.0),
    mix_param(0.5),
    tone_param(1_000.0, 12_000.0, 4_500.0),
    mod_param(0.3),
    ParamDesc {
        key: "rise",
        name: "Rise",
        unit: "ms",
        range: Range::Log {
            min: 80.0,
            max: 2_500.0,
        },
        default: 600.0,
        smoothing_ms: 60.0,
    },
    stepped_param("mode", "Mode", SWELL_MODES, 0.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "swell",
    name: "Swell",
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
        Ctl::Rise,
        Ctl::SwellMode,
    ],
    kind: Kind::Tank,
    insert: Insert::None,
    scale_min: 1.3,
    scale_max: 1.3,
    diff_count: 2,
    diff_g: 0.68,
    lfo_hz: 0.4,
    mod_max_ms: 3.0,
    swell: true,
    bloom: false,
    wet_gain: 1.0,
};
