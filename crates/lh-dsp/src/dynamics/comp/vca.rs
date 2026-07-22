//! VCA compressor (dbx-style): the transparent digital leveler that was the
//! whole `comp` slot before the family split. Full Threshold/Ratio/Attack/
//! Release control over a branching peak detector and a hard-knee dB gain
//! computer. The v7→v8 preset migration lands old flat `comp` slots here, and
//! at these defaults the audio path is the pre-family compressor unchanged.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, RatioMode, VoiceDef, attack_param, blend_param, makeup_param, ratio_param, release_param,
    sc_hpf_param, threshold_param,
};

static PARAMS: [ParamDesc; 7] = [
    threshold_param(-24.0),
    ratio_param(4.0),
    attack_param(0.1, 100.0, 5.0),
    release_param(20.0, 1_000.0, 120.0),
    makeup_param("makeup", "Makeup", 0.0),
    blend_param(),
    sc_hpf_param(),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "vca",
    name: "VCA Comp",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Threshold,
        Ctl::Ratio,
        Ctl::Attack,
        Ctl::Release,
        Ctl::Makeup,
        Ctl::Blend,
        Ctl::ScHpf,
    ],
    knee_db: 0.0, // hard knee — the classic clean VCA curve
    ratio_mode: RatioMode::Knob,
    program_release: false,
    fixed_attack_ms: 5.0,
    release_fast_ms: 0.0,
    release_slow_ms: 0.0,
};
