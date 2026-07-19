//! Digital delay: pristine, full-bandwidth repeats. No feedback saturation,
//! the widest tone sweep, the longest time (up to 2 s), and no modulation —
//! the clean reference echo. The v3→v4 preset migration lands old flat
//! `delay` slots here (its time/feedback/mix carry over unchanged).

use lh_core::{EffectDesc, ParamDesc};

use super::{Ctl, SUBDIVISION, SYNC, VoiceDef, feedback_param, mix_param, time_param, tone_param};

static PARAMS: [ParamDesc; 6] = [
    time_param(2_000.0, 350.0),
    feedback_param(0.9, 0.35),
    mix_param(0.25),
    tone_param(0.7), // bright by default — old `delay` presets stay open
    SUBDIVISION,
    SYNC,
];

pub static DESC: EffectDesc = EffectDesc {
    key: "digital",
    name: "Digital",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Time,
        Ctl::Feedback,
        Ctl::Mix,
        Ctl::Tone,
        Ctl::Subdivision,
        Ctl::Sync,
    ],
    saturate: false,
    drive: 1.0,
    tone_min_hz: 800.0,
    tone_max_hz: 18_000.0,
    lfo_a_hz: 0.0,
    mod_a_ms: 0.0,
    lfo_b_hz: 0.0,
    mod_b_ms: 0.0,
};
