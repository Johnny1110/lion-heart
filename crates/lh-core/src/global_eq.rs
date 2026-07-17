//! Global output EQ state (PRD 003): 8 parametric bands applied to the
//! engine's output stage, after the chain and before the safety limiter.
//!
//! This is **environment correction** (room / monitors / recording chain),
//! not tone: the state is app-global, persisted in
//! `~/.lion-heart/global_eq.json`, and deliberately not part of presets —
//! switching presets never touches it.

use serde::{Deserialize, Serialize};

pub const BAND_COUNT: usize = 8;

pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
/// Bell/shelf gain limits, symmetric.
pub const GAIN_DB_MAX: f32 = 18.0;
pub const Q_MIN: f32 = 0.3;
pub const Q_MAX: f32 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BandKind {
    /// 12 dB/oct highpass; `gain_db` is ignored, `q` shapes the corner.
    LowCut,
    LowShelf,
    Bell,
    HighShelf,
    /// 12 dB/oct lowpass; `gain_db` is ignored, `q` shapes the corner.
    HighCut,
}

impl BandKind {
    pub const ALL: [BandKind; 5] = [
        BandKind::LowCut,
        BandKind::LowShelf,
        BandKind::Bell,
        BandKind::HighShelf,
        BandKind::HighCut,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BandKind::LowCut => "low cut",
            BandKind::LowShelf => "low shelf",
            BandKind::Bell => "bell",
            BandKind::HighShelf => "high shelf",
            BandKind::HighCut => "high cut",
        }
    }

    /// Cut filters have no gain knob — their handle rides the 0 dB line.
    pub fn has_gain(self) -> bool {
        !matches!(self, BandKind::LowCut | BandKind::HighCut)
    }
}

impl std::fmt::Display for BandKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// One EQ band. `Copy` so the whole band travels in one engine message.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub enabled: bool,
    pub kind: BandKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl Band {
    pub const fn bell(freq: f32) -> Self {
        Self {
            enabled: false,
            kind: BandKind::Bell,
            freq,
            gain_db: 0.0,
            q: 0.9,
        }
    }

    /// Clamp all values into their legal ranges (applied on every ingest:
    /// file load, GUI edits, engine messages).
    pub fn clamped(mut self) -> Self {
        self.freq = self.freq.clamp(FREQ_MIN, FREQ_MAX);
        self.gain_db = self.gain_db.clamp(-GAIN_DB_MAX, GAIN_DB_MAX);
        self.q = self.q.clamp(Q_MIN, Q_MAX);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalEqState {
    /// Master toggle; disabled = bit-transparent.
    pub enabled: bool,
    pub bands: [Band; BAND_COUNT],
}

impl Default for GlobalEqState {
    /// A sensible starting layout, all bands disabled — the default output
    /// stage is completely transparent.
    fn default() -> Self {
        let band = |kind, freq, q| Band {
            enabled: false,
            kind,
            freq,
            gain_db: 0.0,
            q,
        };
        Self {
            enabled: true,
            bands: [
                band(BandKind::LowCut, 30.0, 0.707),
                band(BandKind::LowShelf, 80.0, 0.707),
                Band::bell(250.0),
                Band::bell(500.0),
                Band::bell(1_200.0),
                Band::bell(3_000.0),
                band(BandKind::HighShelf, 6_000.0, 0.707),
                band(BandKind::HighCut, 12_000.0, 0.707),
            ],
        }
    }
}

impl GlobalEqState {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("eq state serializes")
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut state: GlobalEqState =
            serde_json::from_str(json).map_err(|e| format!("global eq: {e}"))?;
        for band in &mut state.bands {
            *band = band.clamped();
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_bands_over_the_full_spectrum() {
        let state = GlobalEqState::default();
        assert!(state.enabled);
        assert!(state.bands.iter().all(|b| !b.enabled));
        assert!(state.bands.windows(2).all(|w| w[0].freq < w[1].freq));
        assert_eq!(state.bands[0].kind, BandKind::LowCut);
        assert_eq!(state.bands[7].kind, BandKind::HighCut);
    }

    #[test]
    fn roundtrips_and_clamps_on_load() {
        let mut state = GlobalEqState::default();
        state.bands[2].enabled = true;
        state.bands[2].gain_db = 6.0;
        let back = GlobalEqState::from_json(&state.to_json_pretty()).unwrap();
        assert_eq!(back, state);

        let hot = r#"{"enabled": true, "bands": [
            {"enabled": true, "kind": "bell", "freq": 5.0, "gain_db": 99.0, "q": 100.0},
            {"enabled": false, "kind": "low-cut", "freq": 30.0, "gain_db": 0.0, "q": 0.7},
            {"enabled": false, "kind": "bell", "freq": 250.0, "gain_db": 0.0, "q": 0.9},
            {"enabled": false, "kind": "bell", "freq": 500.0, "gain_db": 0.0, "q": 0.9},
            {"enabled": false, "kind": "bell", "freq": 1200.0, "gain_db": 0.0, "q": 0.9},
            {"enabled": false, "kind": "bell", "freq": 3000.0, "gain_db": 0.0, "q": 0.9},
            {"enabled": false, "kind": "high-shelf", "freq": 6000.0, "gain_db": 0.0, "q": 0.7},
            {"enabled": false, "kind": "high-cut", "freq": 12000.0, "gain_db": 0.0, "q": 0.7}
        ]}"#;
        let state = GlobalEqState::from_json(hot).unwrap();
        assert_eq!(state.bands[0].freq, FREQ_MIN);
        assert_eq!(state.bands[0].gain_db, GAIN_DB_MAX);
        assert_eq!(state.bands[0].q, Q_MAX);
    }
}
