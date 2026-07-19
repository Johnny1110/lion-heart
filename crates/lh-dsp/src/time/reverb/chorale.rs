//! Chorale: a vocal choir in the tail. Two narrow bandpasses at classic
//! vowel formant pairs color the wet — `vowel` morphs A→E→I→O→U, `intensity`
//! blends the choir against the plain tail. The formants sit *outside* the
//! feedback loop, so the resonant boosts can never destabilize the tank.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param, predelay_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(0.5, 8.0, 3.0),
    predelay_param(200.0, 15.0),
    mix_param(0.4),
    tone_param(1_000.0, 8_000.0, 4_000.0),
    mod_param(0.3),
    knob_param("vowel", "Vowel", 0.35),
    knob_param("intensity", "Intensity", 0.6),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "chorale",
    name: "Chorale",
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
        Ctl::Vowel,
        Ctl::Intensity,
    ],
    kind: Kind::Tank,
    insert: Insert::Formant,
    scale_min: 1.1,
    scale_max: 1.1,
    diff_count: 2,
    diff_g: 0.66,
    lfo_hz: 0.3,
    mod_max_ms: 2.0,
    swell: false,
    bloom: false,
    wet_gain: 1.0,
};
