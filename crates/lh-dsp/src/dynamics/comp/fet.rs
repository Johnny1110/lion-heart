//! FET compressor (UREI 1176-style): a fast FET peak limiter. Microsecond
//! attack (fast enough to grab transients — or shape them), a hard knee, and a
//! **stepped** ratio (4:1 / 8:1 / 12:1 / 20:1 / All). The "all-buttons-in"
//! step drops the threshold under the 20:1 curve for the notorious aggressive
//! pump. Punchy, bright, aggressive. `threshold` stands in for the hardware's
//! INPUT-into-a-fixed-threshold drive (ADR 025).

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, RatioMode, VoiceDef, attack_param, blend_param, makeup_param, ratio_step_param,
    release_param, sc_hpf_param, threshold_param,
};

static PARAMS: [ParamDesc; 7] = [
    threshold_param(-20.0),
    attack_param(0.02, 0.8, 0.05), // 20 µs – 800 µs, the FET's signature speed
    release_param(50.0, 1_100.0, 200.0),
    ratio_step_param(0.0), // 4:1
    makeup_param("makeup", "Makeup", 0.0),
    blend_param(),
    sc_hpf_param(),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "fet",
    name: "FET Comp",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[
        Ctl::Threshold,
        Ctl::Attack,
        Ctl::Release,
        Ctl::RatioStep,
        Ctl::Makeup,
        Ctl::Blend,
        Ctl::ScHpf,
    ],
    knee_db: 0.0, // hard knee — aggressive, immediate
    ratio_mode: RatioMode::Stepped,
    program_release: false,
    fixed_attack_ms: 0.05,
    release_fast_ms: 0.0,
    release_slow_ms: 0.0,
};
