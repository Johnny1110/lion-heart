//! Room: studio ambience up to a night-club box — a small, fast tank whose
//! `diffusion` knob sweeps the input scattering (bare walls ⇄ furniture and
//! people), the wet arriving noticeably earlier than hall's.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param, predelay_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(0.15, 3.0, 0.7),
    predelay_param(120.0, 10.0),
    mix_param(0.28),
    tone_param(1_500.0, 12_000.0, 6_000.0),
    mod_param(0.1),
    knob_param("size", "Size", 0.5),
    knob_param("diffusion", "Diffusion", 0.6),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "room",
    name: "Room",
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
        Ctl::Diffusion,
    ],
    kind: Kind::Tank,
    insert: Insert::None,
    scale_min: 0.3,
    scale_max: 0.85,
    diff_count: 2,
    diff_g: 0.6, // baseline; the diffusion knob overrides in the hot loop
    lfo_hz: 0.5,
    mod_max_ms: 1.5,
    swell: false,
    bloom: false,
    wet_gain: 1.0,
};
