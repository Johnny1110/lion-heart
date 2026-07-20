//! Vintage delay: bucket-brigade voiced. Dark and narrow (the lowest tone
//! range), with harder feedback compression — the gooey, murky repeats of an
//! analog BBD — and a single chorus-y **Mod** LFO. The time range is short
//! (up to 600 ms, like real BBD chips), and feedback past unity self-
//! oscillates into a thick bounded drone.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, SUBDIVISION, SYNC, VoiceDef, depth_param, feedback_param, mix_param, time_param,
    tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    time_param(600.0, 300.0),
    feedback_param(1.05, 0.4),
    mix_param(0.28),
    tone_param(0.3), // dark BBD voicing
    depth_param("mod", "Mod", 0.3),
    SUBDIVISION,
    SYNC,
];

pub static DESC: EffectDesc = EffectDesc {
    key: "vintage",
    name: "Vintage",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Time,
        Ctl::Feedback,
        Ctl::Mix,
        Ctl::Tone,
        Ctl::ModA, // Mod
        Ctl::Subdivision,
        Ctl::Sync,
    ],
    saturate: true,
    drive: 2.2,
    tone_min_hz: 400.0,
    tone_max_hz: 5_000.0,
    lfo_a_hz: 0.9,
    mod_a_ms: 6.0,
    lfo_b_hz: 0.0,
    mod_b_ms: 0.0,
};
