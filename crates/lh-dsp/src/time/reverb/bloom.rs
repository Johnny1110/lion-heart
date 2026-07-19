//! Bloom: the '90s trick of stacking diffusion until the reverb swells up
//! behind the note. A regenerative loop around the (long, four-stage)
//! diffused feed — `feedback` is the loop gain (capped below unity, so it
//! always converges), `length` the loop time — makes the attack smear and
//! build before the tank takes over.

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param, predelay_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(1.0, 15.0, 5.0),
    predelay_param(250.0, 20.0),
    mix_param(0.5),
    tone_param(800.0, 10_000.0, 3_500.0),
    mod_param(0.25),
    ParamDesc {
        key: "feedback",
        name: "Feedback",
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 0.85, // bloom-loop gain: strictly below unity by design
        },
        default: 0.35,
        smoothing_ms: 30.0,
    },
    knob_param("length", "Length", 0.5),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "bloom",
    name: "Bloom",
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
        Ctl::Feedback,
        Ctl::Length,
    ],
    kind: Kind::Tank,
    insert: Insert::None,
    scale_min: 1.5,
    scale_max: 1.5,
    diff_count: 4,
    diff_g: 0.7,
    lfo_hz: 0.3,
    mod_max_ms: 4.0,
    swell: false,
    bloom: true,
    wet_gain: 0.9,
};
