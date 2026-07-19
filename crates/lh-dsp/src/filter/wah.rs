//! **wah** — the manual wah (PRD 008): a treadle position, not an envelope,
//! decides where the filter sits. Built for an expression pedal — `pos` is
//! the CC landing zone, smoothed 25 ms so a 7-bit controller staircase
//! glides instead of zipping.
//!
//! Voiced narrower than the autowah: 350 Hz → 2.2 kHz is the Crybaby
//! throat — a manual wah is a vowel, not a funk filter — and the default
//! resonance sits higher (q 6) for the vocal formant. No `direction` knob:
//! a reversed pedal is the mapping's job (min > max in `midi.json`).

use lh_core::{EffectDesc, ParamDesc, Range};

use super::{Ctl, MODES, PedalDef};

static PARAMS: [ParamDesc; 4] = [
    ParamDesc {
        key: "pos",
        name: "Pos",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.5,
        smoothing_ms: 25.0,
    },
    ParamDesc {
        key: "q",
        name: "Q",
        unit: "",
        range: Range::Log {
            min: 1.5,
            max: 12.0,
        },
        default: 6.0,
        smoothing_ms: 60.0,
    },
    ParamDesc {
        key: "mode",
        name: "Mode",
        unit: "",
        range: Range::Stepped { labels: MODES },
        default: 0.0, // lowpass: the classic wah keeps its body below the peak
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 1.0,
        smoothing_ms: 30.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "wah",
    name: "Wah",
    params: &PARAMS,
};

pub const PEDAL: PedalDef = PedalDef {
    desc: &DESC,
    controls: &[Ctl::Pos, Ctl::Q, Ctl::Mode, Ctl::Mix],
    fc_min_hz: 350.0,
    fc_max_hz: 2_200.0,
    follower: false,
};
