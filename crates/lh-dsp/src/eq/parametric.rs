//! The parametric pedal (PRD 011): the global output EQ's 8-band engine
//! (PRD 003) offered as a chain pedal, so the same visual EQ can sit at any
//! position of the chain (and more than once — slots are instances).
//!
//! The DSP **is** [`super::global::GlobalEq`] — per-band wet crossfades,
//! log-domain freq smoothing, settled-skip coefficient rebuilds, and
//! bit-transparency with every band off are all inherited, not rewritten.
//! This module is the mapping between the pedal's flat parameter space
//! (8 bands × on/type/freq/gain/q, PRD 001 faceplate rules) and the core's
//! [`Band`] model. Slot bypass is the engine's job; the core's master stays
//! pinned at 1.0 and is not exposed.

use lh_core::global_eq::{
    BAND_COUNT, Band, BandKind, FREQ_MAX, FREQ_MIN, GAIN_DB_MAX, GlobalEqState, Q_MAX, Q_MIN,
};
use lh_core::{EffectDesc, ParamDesc, Range};

use super::global::GlobalEq;

/// Params per band: on / type / freq / gain / q, in that order.
pub const BAND_PARAMS: usize = 5;

/// `b*_type` labels, aligned with [`BandKind::ALL`] (pinned by test).
pub const KIND_LABELS: [&str; 5] = ["low cut", "low shelf", "bell", "high shelf", "high cut"];
const ON_LABELS: [&str; 2] = ["off", "on"];

/// One row per band: key prefix, display prefix, default kind index (into
/// [`BandKind::ALL`]), default freq, default Q — the same transparent layout
/// as [`GlobalEqState::default`] (pinned by test).
macro_rules! band_params {
    ($(($prefix:literal, $label:literal, $kind:literal, $freq:literal, $q:literal)),* $(,)?) => {
        [
            $(
                ParamDesc {
                    key: concat!($prefix, "_on"),
                    name: concat!($label, " On"),
                    unit: "",
                    range: Range::Stepped { labels: &ON_LABELS },
                    default: 0.0,
                    smoothing_ms: 0.0,
                },
                ParamDesc {
                    key: concat!($prefix, "_type"),
                    name: concat!($label, " Type"),
                    unit: "",
                    range: Range::Stepped {
                        labels: &KIND_LABELS,
                    },
                    default: $kind,
                    smoothing_ms: 0.0,
                },
                ParamDesc {
                    key: concat!($prefix, "_freq"),
                    name: concat!($label, " Freq"),
                    unit: "Hz",
                    range: Range::Log {
                        min: FREQ_MIN,
                        max: FREQ_MAX,
                    },
                    default: $freq,
                    smoothing_ms: 30.0,
                },
                ParamDesc {
                    key: concat!($prefix, "_gain"),
                    name: concat!($label, " Gain"),
                    unit: "dB",
                    range: Range::Linear {
                        min: -GAIN_DB_MAX,
                        max: GAIN_DB_MAX,
                    },
                    default: 0.0,
                    smoothing_ms: 30.0,
                },
                ParamDesc {
                    key: concat!($prefix, "_q"),
                    name: concat!($label, " Q"),
                    unit: "",
                    range: Range::Log {
                        min: Q_MIN,
                        max: Q_MAX,
                    },
                    default: $q,
                    smoothing_ms: 30.0,
                },
            )*
        ]
    };
}

static PARAMS: [ParamDesc; BAND_COUNT * BAND_PARAMS] = band_params![
    ("b1", "B1", 0.0, 30.0, 0.707),
    ("b2", "B2", 1.0, 80.0, 0.707),
    ("b3", "B3", 2.0, 250.0, 0.9),
    ("b4", "B4", 2.0, 500.0, 0.9),
    ("b5", "B5", 2.0, 1_200.0, 0.9),
    ("b6", "B6", 2.0, 3_000.0, 0.9),
    ("b7", "B7", 3.0, 6_000.0, 0.707),
    ("b8", "B8", 4.0, 12_000.0, 0.707),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "parametric",
    name: "Parametric",
    params: &PARAMS,
};

/// Rebuild the band model from the pedal's real-world values in param order
/// (e.g. the GUI's slot mirror) — curve and handles then come from the same
/// [`Band`] math the audio path runs. Missing values keep the defaults.
pub fn bands_from_reals(reals: &[f32]) -> [Band; BAND_COUNT] {
    let mut bands = GlobalEqState::default().bands;
    for (i, band) in bands.iter_mut().enumerate() {
        let at = |field: usize| reals.get(i * BAND_PARAMS + field).copied();
        if let Some(v) = at(0) {
            band.enabled = v >= 0.5;
        }
        if let Some(v) = at(1) {
            band.kind = BandKind::ALL[(v as usize).min(BandKind::ALL.len() - 1)];
        }
        if let Some(v) = at(2) {
            band.freq = v;
        }
        if let Some(v) = at(3) {
            band.gain_db = v;
        }
        if let Some(v) = at(4) {
            band.q = v;
        }
        *band = band.clamped();
    }
    bands
}

pub struct Parametric {
    core: GlobalEq,
    /// Target band values (the core smooths toward them).
    bands: [Band; BAND_COUNT],
}

impl Default for Parametric {
    fn default() -> Self {
        Self::new()
    }
}

impl Parametric {
    pub fn new() -> Self {
        Self {
            // The core's own default state is exactly ours: transparent
            // layout, master enabled (and never touched again).
            core: GlobalEq::new(),
            bands: GlobalEqState::default().bands,
        }
    }

    pub fn prepare(&mut self, sample_rate: u32) {
        self.core.prepare(sample_rate);
    }

    pub fn reset(&mut self) {
        self.core.reset();
    }

    pub fn set_param(&mut self, index: usize, normalized: f32) {
        let Some(param) = PARAMS.get(index) else {
            return;
        };
        let real = param.range.to_real(normalized);
        let band = index / BAND_PARAMS;
        let b = &mut self.bands[band];
        match index % BAND_PARAMS {
            0 => b.enabled = real >= 0.5,
            1 => b.kind = BandKind::ALL[(real as usize).min(BandKind::ALL.len() - 1)],
            2 => b.freq = real,
            3 => b.gain_db = real,
            _ => b.q = real,
        }
        self.core.set_band(band, *b);
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.core.process(left, right);
    }

    /// The current targets as core state (`enabled` = true; the master is
    /// the slot's bypass, not a pedal param) — for response probes.
    pub fn state(&self) -> GlobalEqState {
        GlobalEqState {
            enabled: true,
            bands: self.bands,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_mirror_the_global_default_layout() {
        let state = GlobalEqState::default();
        assert_eq!(PARAMS.len(), BAND_COUNT * BAND_PARAMS);
        for (i, band) in state.bands.iter().enumerate() {
            let p = |field: usize| &PARAMS[i * BAND_PARAMS + field];
            assert_eq!(p(0).default, 0.0, "band {i} ships off");
            let kind = BandKind::ALL[p(1).default as usize];
            assert_eq!(kind, band.kind, "band {i} kind");
            assert_eq!(p(2).default, band.freq, "band {i} freq");
            assert_eq!(p(3).default, band.gain_db, "band {i} gain");
            assert_eq!(p(4).default, band.q, "band {i} q");
            // Key naming is the GUI/REPL/MIDI contract: b{n}_{field}.
            let n = i + 1;
            assert_eq!(p(0).key, format!("b{n}_on"));
            assert_eq!(p(1).key, format!("b{n}_type"));
            assert_eq!(p(2).key, format!("b{n}_freq"));
            assert_eq!(p(3).key, format!("b{n}_gain"));
            assert_eq!(p(4).key, format!("b{n}_q"));
        }
    }

    #[test]
    fn kind_labels_align_with_band_kind_all() {
        for (label, kind) in KIND_LABELS.iter().zip(BandKind::ALL) {
            assert_eq!(*label, kind.label());
        }
    }

    #[test]
    fn bands_from_reals_roundtrips_param_defaults() {
        let reals: Vec<f32> = PARAMS.iter().map(|p| p.default).collect();
        assert_eq!(bands_from_reals(&reals), GlobalEqState::default().bands);
        // A sparse mirror keeps defaults for the missing tail.
        assert_eq!(bands_from_reals(&[]), GlobalEqState::default().bands);
        // Field routing: enable b3 as a 1 kHz +6 dB bell.
        let mut edited = reals.clone();
        edited[2 * BAND_PARAMS] = 1.0;
        edited[2 * BAND_PARAMS + 2] = 1_000.0;
        edited[2 * BAND_PARAMS + 3] = 6.0;
        let bands = bands_from_reals(&edited);
        assert!(bands[2].enabled);
        assert_eq!(bands[2].kind, BandKind::Bell);
        assert_eq!(bands[2].freq, 1_000.0);
        assert_eq!(bands[2].gain_db, 6.0);
    }
}
