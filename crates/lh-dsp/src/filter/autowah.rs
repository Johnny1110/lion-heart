//! **autowah** — an envelope filter (Mu-Tron style): how hard you pick is
//! where the filter sits (PRD 007).
//!
//! - **Envelope follower**: |mono input| × `sens` pre-gain through an
//!   asymmetric one-pole — fixed ~2 ms attack (the quack must speak on the
//!   transient), `decay`-knob release (60–600 ms — the feel knob: how fast
//!   the filter falls back while a note rings).
//! - **Sweep**: the envelope maps geometrically over 180 Hz → 2.4 kHz;
//!   `direction` flips it (down = the reverse quack: hit hard, sweep low).

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Ctl, DIRECTIONS, MODES, PedalDef};

static PARAMS: [ParamDesc; 6] = [
    ParamDesc {
        key: "sens",
        name: "Sens",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.5,
        smoothing_ms: 50.0,
    },
    ParamDesc {
        key: "q",
        name: "Q",
        unit: "",
        range: Range::Log {
            min: 1.5,
            max: 12.0,
        },
        default: 4.0,
        smoothing_ms: 60.0,
    },
    ParamDesc {
        key: "decay",
        name: "Decay",
        unit: "ms",
        range: Range::Log {
            min: 60.0,
            max: 600.0,
        },
        default: 180.0,
        smoothing_ms: 60.0,
    },
    ParamDesc {
        key: "mode",
        name: "Mode",
        unit: "",
        range: Range::Stepped { labels: MODES },
        default: 0.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "direction",
        name: "Direction",
        unit: "",
        range: Range::Stepped { labels: DIRECTIONS },
        default: 0.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 1.0, // the classic envelope filter is fully wet
        smoothing_ms: 30.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "autowah",
    name: "Auto Wah",
    params: &PARAMS,
};

pub const PEDAL: PedalDef = PedalDef {
    desc: &DESC,
    controls: &[
        Ctl::Sens,
        Ctl::Q,
        Ctl::Decay,
        Ctl::Mode,
        Ctl::Direction,
        Ctl::Mix,
    ],
    fc_min_hz: 180.0,
    fc_max_hz: 2_400.0,
    follower: true,
};
