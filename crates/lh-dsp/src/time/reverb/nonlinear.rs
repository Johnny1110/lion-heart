//! Nonlinear: physics-defying envelopes — `gate` (flat, then cut),
//! `reverse` (rising into the cut), `swoosh` (an arch). Not a decay loop at
//! all: a feedback-free fan of 24 jittered taps over a `decay`-long window,
//! each weighted by the shape law. The tail ends because the window ends.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, NONLINEAR_SHAPES, VoiceDef, decay_param, mix_param, predelay_param,
    stepped_param, tone_param,
};

static PARAMS: [ParamDesc; 5] = [
    decay_param(0.12, 1.5, 0.45),
    predelay_param(120.0, 0.0),
    mix_param(0.5),
    tone_param(1_500.0, 12_000.0, 6_000.0),
    stepped_param("shape", "Shape", NONLINEAR_SHAPES, 0.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "nonlinear",
    name: "Nonlinear",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[Ctl::Decay, Ctl::Predelay, Ctl::Mix, Ctl::Tone, Ctl::Shape],
    kind: Kind::Shaped,
    insert: Insert::None,
    scale_min: 1.0,
    scale_max: 1.0,
    diff_count: 2,
    diff_g: 0.65,
    lfo_hz: 0.0,
    mod_max_ms: 0.0,
    swell: false,
    bloom: false,
    wet_gain: 0.35, // 24-tap fan normalization
};
