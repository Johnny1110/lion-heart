//! Spring: mechanical tension. `dwell` drives the input into a soft clip
//! (hit it harder, it growls back), then 1–3 parallel detuned chirp
//! cascades — 2nd-order allpasses whose group delay peaks near 2–3 kHz —
//! give the dispersive "boing" before a tiny, dark tank. No mod knob: the
//! wobble of a real pan is in the chirps, not a chorus.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, predelay_param, stepped_param,
    tone_param,
};

pub const SPRING_COUNTS: &[&str] = &["1", "2", "3"];

static PARAMS: [ParamDesc; 6] = [
    decay_param(0.5, 4.0, 1.7),
    predelay_param(120.0, 8.0),
    mix_param(0.32),
    tone_param(1_200.0, 6_000.0, 3_200.0),
    knob_param("dwell", "Dwell", 0.35),
    stepped_param("springs", "Springs", SPRING_COUNTS, 1.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "spring",
    name: "Spring",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Decay,
        Ctl::Predelay,
        Ctl::Mix,
        Ctl::Tone,
        Ctl::Dwell,
        Ctl::Springs,
    ],
    kind: Kind::Tank,
    insert: Insert::Chirp,
    scale_min: 0.35,
    scale_max: 0.35,
    diff_count: 2,
    diff_g: 0.55,
    lfo_hz: 0.0,
    mod_max_ms: 0.0,
    swell: false,
    bloom: false,
    wet_gain: 1.15,
};
