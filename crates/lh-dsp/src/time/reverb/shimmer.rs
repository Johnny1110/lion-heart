//! Shimmer: pitch-shifted regeneration. Each pass around the tank, part of
//! the tail re-enters through a granular shifter (`interval`: +octave,
//! +5th, −octave, or a dual octave+5th stack), so the wash climbs into
//! unearthly overtones. The re-entry is soft-clipped: a cranked `amount`
//! over a long decay drones majestically instead of running away — the
//! same bound as the delay family's self-oscillation.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, INTERVALS, Insert, Kind, VoiceDef, decay_param, knob_param, mix_param, mod_param,
    predelay_param, stepped_param, tone_param,
};

static PARAMS: [ParamDesc; 7] = [
    decay_param(1.0, 15.0, 6.0),
    predelay_param(250.0, 20.0),
    mix_param(0.35),
    tone_param(1_500.0, 14_000.0, 7_000.0),
    mod_param(0.3),
    knob_param("amount", "Amount", 0.5),
    stepped_param("interval", "Interval", INTERVALS, 0.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "shimmer",
    name: "Shimmer",
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
        Ctl::Amount,
        Ctl::Interval,
    ],
    kind: Kind::Tank,
    insert: Insert::Shimmer,
    scale_min: 1.4,
    scale_max: 1.4,
    diff_count: 2,
    diff_g: 0.68,
    lfo_hz: 0.4,
    mod_max_ms: 3.0,
    swell: false,
    bloom: false,
    wet_gain: 0.95,
};
