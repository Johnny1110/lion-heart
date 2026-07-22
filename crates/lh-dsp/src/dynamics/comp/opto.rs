//! Opto compressor (Teletronix LA-2A-style): a slow, round optical leveler.
//! The optical cell owns attack (fixed, slow) and ratio (rising toward
//! limiting as you push harder), so the faceplate is just Peak Reduction and
//! Gain. Its signature is the **program-dependent release** — a two-stage
//! recovery where deep gain reduction lets go far more slowly than a light
//! touch — plus a soft knee. Sticky, musical, forgiving.

use lh_core::{EffectDesc, ParamDesc};

use super::{
    Ctl, RatioMode, VoiceDef, blend_param, makeup_param, peak_reduction_param, sc_hpf_param,
};

static PARAMS: [ParamDesc; 4] = [
    peak_reduction_param(0.4),
    makeup_param("gain", "Gain", 0.0),
    blend_param(),
    sc_hpf_param(),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "opto",
    name: "Opto Comp",
    params: &PARAMS,
};

pub const VOICE: VoiceDef = VoiceDef {
    desc: &DESC,
    controls: &[Ctl::PeakReduction, Ctl::Makeup, Ctl::Blend, Ctl::ScHpf],
    knee_db: 12.0, // soft, rounded onset
    ratio_mode: RatioMode::Rising {
        base: 2.5,
        top: 8.0,
    },
    program_release: true,
    fixed_attack_ms: 10.0, // the T4 cell's slow, fixed attack
    release_fast_ms: 100.0,
    release_slow_ms: 1_500.0,
};
