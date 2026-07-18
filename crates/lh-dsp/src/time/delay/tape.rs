//! Tape delay: warm and alive. The feedback write is soft-clipped so repeats
//! compress and thicken instead of clipping, and each pass loses more high
//! end (the tone lowpass sits in the loop). Two LFOs give the classic tape
//! movement — a slow **Wow** and a fast **Flutter**, both a touch on by
//! default for that hint of chorus. Feedback reaches unity, so a cranked
//! setting self-oscillates into a bounded drone rather than dying.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, SUBDIVISION, VoiceDef, depth_param, feedback_param, mix_param, time_param, tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    time_param(1_200.0, 350.0),
    feedback_param(1.0, 0.4),
    mix_param(0.28),
    tone_param(0.45), // warmer than digital
    depth_param("wow", "Wow", 0.25),
    depth_param("flutter", "Flutter", 0.2),
    SUBDIVISION,
];

pub static DESC: EffectDesc = EffectDesc {
    key: "tape",
    name: "Tape",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Time,
        Ctl::Feedback,
        Ctl::Mix,
        Ctl::Tone,
        Ctl::ModA, // Wow
        Ctl::ModB, // Flutter
        Ctl::Subdivision,
    ],
    saturate: true,
    drive: 1.4,
    tone_min_hz: 600.0,
    tone_max_hz: 9_000.0,
    lfo_a_hz: 0.55, // wow
    mod_a_ms: 7.0,
    lfo_b_hz: 7.0, // flutter
    mod_b_ms: 1.2,
};
