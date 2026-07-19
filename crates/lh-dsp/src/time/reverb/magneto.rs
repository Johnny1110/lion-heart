//! Magneto: an old-school multi-head drum echo in front of the tank. 1–4
//! playback heads at `spacing` intervals, `repeats` feeding the last head
//! back (soft-clipped, darkened in-loop — tape, not RAM), the head bus
//! washing into the reverb behind it. `mod` is wow on the head reads.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{
    Ctl, HEAD_LABELS, Insert, Kind, VoiceDef, decay_param, mix_param, mod_param, stepped_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(0.5, 8.0, 2.5),
    ParamDesc {
        key: "spacing",
        name: "Spacing",
        unit: "ms",
        range: Range::Log {
            min: 60.0,
            max: 400.0,
        },
        default: 140.0,
        smoothing_ms: 150.0,
    },
    mix_param(0.35),
    tone_param(1_000.0, 8_000.0, 3_500.0),
    mod_param(0.25),
    ParamDesc {
        key: "repeats",
        name: "Repeats",
        unit: "",
        range: Range::Linear { min: 0.0, max: 0.9 },
        default: 0.4,
        smoothing_ms: 20.0,
    },
    stepped_param("heads", "Heads", HEAD_LABELS, 2.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "magneto",
    name: "Magneto",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Decay,
        Ctl::Spacing,
        Ctl::Mix,
        Ctl::Tone,
        Ctl::Mod,
        Ctl::Repeats,
        Ctl::Heads,
    ],
    kind: Kind::Magneto,
    insert: Insert::None,
    scale_min: 1.0,
    scale_max: 1.0,
    diff_count: 2,
    diff_g: 0.65,
    lfo_hz: 0.45, // wow
    mod_max_ms: 2.2,
    swell: false,
    bloom: false,
    wet_gain: 0.9,
};
