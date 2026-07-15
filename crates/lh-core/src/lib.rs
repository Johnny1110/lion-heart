//! Core vocabulary of Lion-Heart: parameter identities, value mappings,
//! effect descriptors, and the preset schema. No I/O, no threads — everything
//! here is plain data, shared by the DSP, engine, UI, and MIDI layers.
//!
//! Convention (white paper §4.3): parameters cross module boundaries as
//! **normalized** values in `0.0..=1.0`; real-world units live behind a
//! [`Range`] mapping owned by the parameter's [`ParamDesc`]. Presets store
//! **real** values keyed by names, so files stay meaningful to humans and
//! robust against parameter reordering.

pub mod preset;

/// Decibels → linear amplitude.
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear amplitude → decibels, floored at -120 dB so silence stays finite.
pub fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.abs().max(1e-6).log10()
}

/// Stable address of one parameter: `slot` indexes the chain position,
/// `param` indexes into that effect's descriptor. Survives serialization as
/// long as chain layout versions are respected (preset schema, M3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamId {
    pub slot: u8,
    pub param: u8,
}

/// Mapping between normalized `0..=1` and a real-world value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Range {
    Linear {
        min: f32,
        max: f32,
    },
    /// Geometric mapping for frequencies and times; `min` must be > 0.
    Log {
        min: f32,
        max: f32,
    },
}

impl Range {
    pub fn min(&self) -> f32 {
        match *self {
            Range::Linear { min, .. } | Range::Log { min, .. } => min,
        }
    }

    pub fn max(&self) -> f32 {
        match *self {
            Range::Linear { max, .. } | Range::Log { max, .. } => max,
        }
    }

    pub fn clamp(&self, real: f32) -> f32 {
        real.clamp(self.min(), self.max())
    }

    pub fn to_real(&self, normalized: f32) -> f32 {
        let n = normalized.clamp(0.0, 1.0);
        match *self {
            Range::Linear { min, max } => min + (max - min) * n,
            Range::Log { min, max } => min * (max / min).powf(n),
        }
    }

    pub fn to_norm(&self, real: f32) -> f32 {
        let r = self.clamp(real);
        let n = match *self {
            Range::Linear { min, max } => (r - min) / (max - min),
            Range::Log { min, max } => (r / min).ln() / (max / min).ln(),
        };
        n.clamp(0.0, 1.0)
    }
}

/// Static description of one parameter.
#[derive(Debug)]
pub struct ParamDesc {
    /// Machine name used in CLI/presets, e.g. `"threshold"`.
    pub key: &'static str,
    /// Human name, e.g. `"Threshold"`.
    pub name: &'static str,
    /// Display unit, e.g. `"dB"`, `"ms"`, `"Hz"`, `""`.
    pub unit: &'static str,
    pub range: Range,
    /// Default as a real-world value (inside `range`).
    pub default: f32,
    /// Smoothing time constant applied by the effect; 0 = snap.
    pub smoothing_ms: f32,
}

impl ParamDesc {
    pub fn default_norm(&self) -> f32 {
        self.range.to_norm(self.default)
    }
}

/// Static description of one effect kind.
#[derive(Debug)]
pub struct EffectDesc {
    /// Machine name used in CLI/presets, e.g. `"drive"`.
    pub key: &'static str,
    pub name: &'static str,
    pub params: &'static [ParamDesc],
}

impl EffectDesc {
    pub fn param_index(&self, key: &str) -> Option<usize> {
        self.params.iter().position(|p| p.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_conversions_roundtrip() {
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!((lin_to_db(db_to_lin(-12.0)) - -12.0).abs() < 1e-3);
        assert!(lin_to_db(0.0) <= -120.0 + 1e-3);
    }

    #[test]
    fn linear_range_maps_and_clamps() {
        let r = Range::Linear {
            min: -80.0,
            max: -20.0,
        };
        assert!((r.to_real(0.0) - -80.0).abs() < 1e-6);
        assert!((r.to_real(1.0) - -20.0).abs() < 1e-6);
        assert!((r.to_real(0.5) - -50.0).abs() < 1e-6);
        assert!((r.to_norm(-50.0) - 0.5).abs() < 1e-6);
        // Out-of-range values clamp instead of extrapolating.
        assert!((r.to_norm(0.0) - 1.0).abs() < 1e-6);
        assert!((r.to_real(2.0) - -20.0).abs() < 1e-6);
    }

    #[test]
    fn log_range_midpoint_is_geometric_mean() {
        let r = Range::Log {
            min: 100.0,
            max: 10_000.0,
        };
        assert!((r.to_real(0.5) - 1_000.0).abs() < 1.0);
        assert!((r.to_norm(1_000.0) - 0.5).abs() < 1e-4);
        assert!((r.to_norm(r.to_real(0.3)) - 0.3).abs() < 1e-5);
    }
}
