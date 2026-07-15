//! The NAM amp block: loads community `.nam` captures (WaveNet/LSTM) and runs
//! them through `nam-rs` (pure Rust, RT-safe inference). This crate is the
//! seam described in white paper §5.3 — if `nam-rs` ever falls short, an FFI
//! binding to NeuralAmpModelerCore replaces [`NamAsset`] behind the same
//! effect, and callers never notice (that switch requires an ADR).

use std::path::Path;

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};
use lh_dsp::Effect;
use lh_dsp::smooth::Smoothed;
use lh_dsp::swap::{AssetHandle, AssetSlot, asset_channel};
use thiserror::Error;

/// Captures are normalized so different models land at a comparable loudness
/// instead of whatever level the capture rig happened to produce.
pub const NORMALIZE_TARGET_DB: f32 = -18.0;

#[derive(Debug, Error)]
pub enum NamError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },

    #[error(
        "model expects {model} Hz but the engine runs at {engine} Hz — NAM models are \
         rate-locked (white paper §5.3). Restart with --sample-rate {model}, or use a \
         {engine} Hz capture."
    )]
    RateMismatch { model: u32, engine: u32 },

    #[error("not a usable .nam model: {0}")]
    Model(String),
}

/// A loaded, runnable capture plus its calibration.
pub struct NamAsset {
    pub model: nam_rs::Model,
    /// Output scale folding the capture's loudness metadata onto
    /// [`NORMALIZE_TARGET_DB`]; 1.0 when the file carries no loudness.
    pub base_gain: f32,
}

/// Human-facing facts about a loaded capture.
#[derive(Debug, Clone)]
pub struct NamInfo {
    pub architecture: String,
    pub sample_rate: u32,
    pub loudness_db: Option<f32>,
    pub normalized: bool,
}

/// Parse and validate a `.nam` file into an installable asset.
/// Control-thread only (allocates); the returned asset is RT-safe to run.
pub fn load_nam_file(path: &Path, engine_rate: u32) -> Result<(Box<NamAsset>, NamInfo), NamError> {
    let nam = nam_rs::NamModel::from_file(path).map_err(|e| NamError::Model(e.to_string()))?;

    let model_rate = nam.expected_sample_rate().round() as u32;
    if model_rate != engine_rate {
        return Err(NamError::RateMismatch {
            model: model_rate,
            engine: engine_rate,
        });
    }

    let model = nam_rs::Model::from_nam(&nam).map_err(|e| NamError::Model(e.to_string()))?;

    let loudness_db = nam.loudness();
    let base_gain = loudness_db
        .map(|l| db_to_lin(NORMALIZE_TARGET_DB - l))
        .unwrap_or(1.0);

    let info = NamInfo {
        architecture: nam.architecture.clone(),
        sample_rate: model_rate,
        loudness_db,
        normalized: loudness_db.is_some(),
    };
    Ok((Box::new(NamAsset { model, base_gain }), info))
}

static PARAMS: [ParamDesc; 2] = [
    ParamDesc {
        key: "gain",
        name: "Gain",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 20.0,
    },
    ParamDesc {
        key: "level",
        name: "Level",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 20.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "amp",
    name: "NAM Amp",
    params: &PARAMS,
};

/// The amp slot in the chain. Passes dry until a capture is installed.
pub struct NamAmp {
    slot: AssetSlot<NamAsset>,
    gain: Smoothed,
    level: Smoothed,
}

impl NamAmp {
    pub fn new() -> (Self, AssetHandle<NamAsset>) {
        let (slot, handle) = asset_channel();
        (
            Self {
                slot,
                gain: Smoothed::new(db_to_lin(PARAMS[0].default)),
                level: Smoothed::new(db_to_lin(PARAMS[1].default)),
            },
            handle,
        )
    }
}

impl Effect for NamAmp {
    fn descriptor(&self) -> &'static EffectDesc {
        &DESC
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.gain.configure(PARAMS[0].smoothing_ms, sample_rate);
        self.level.configure(PARAMS[1].smoothing_ms, sample_rate);
        self.gain.snap_to_target();
        self.level.snap_to_target();
    }

    fn reset(&mut self) {
        if let Some(asset) = self.slot.get_mut() {
            asset.model.reset();
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match index {
            0 => self
                .gain
                .set_target(db_to_lin(PARAMS[0].range.to_real(normalized))),
            1 => self
                .level
                .set_target(db_to_lin(PARAMS[1].range.to_real(normalized))),
            _ => {}
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        self.slot.tick();
        for x in block.iter_mut() {
            *x *= self.gain.tick();
        }
        let base = match self.slot.get_mut() {
            Some(asset) => {
                asset.model.process_buffer(block);
                asset.base_gain
            }
            None => 1.0,
        };
        for x in block.iter_mut() {
            *x *= base * self.level.tick();
        }
    }
}
