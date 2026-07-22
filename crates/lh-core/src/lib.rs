//! Core vocabulary of Lion-Heart: parameter identities, value mappings,
//! effect descriptors, and the preset schema. No I/O, no threads — everything
//! here is plain data, shared by the DSP, engine, UI, and MIDI layers.
//!
//! Convention (white paper §4.3): parameters cross module boundaries as
//! **normalized** values in `0.0..=1.0`; real-world units live behind a
//! [`Range`] mapping owned by the parameter's [`ParamDesc`]. Presets store
//! **real** values keyed by names, so files stay meaningful to humans and
//! robust against parameter reordering.

pub mod drive_law;
pub mod global_eq;
pub mod preset;
pub mod tempo;

/// The default pedalboard, as family keys in processing order. The single
/// source of truth for "the full rig": the app's session registry and the
/// plugin's fixed chain are both pinned to it by tests, so the two binaries
/// cannot drift apart.
///
/// `filter` sits before the compressor on purpose: its envelope follower
/// feeds on playing dynamics, which the compressor exists to squash. `power`
/// (the hand-written valve power stage, PRD 017) sits after `amp` and before
/// `cab` — post-preamp, pre-speaker, exactly where a real power section lives.
pub const DEFAULT_CHAIN: [&str; 12] = [
    "gate", "filter", "comp", "drive", "amp", "power", "eq", "mod", "delay", "reverb", "cab",
    "limiter",
];

/// Whether a family's slot ships **active** on the default board. Almost
/// everything does — their defaults are transparent (gate low, comp gentle,
/// limiter above the music). Two families ship **bypassed**: a `filter` has no
/// transparent knob position (it colors the signal wherever it sits) and lights
/// up when the player engages it (PRD 007); the `power` stage would
/// double-colour a full-amp NAM capture that already contains a power section,
/// so preamp-only players light it deliberately (PRD 017).
pub fn default_active(family_key: &str) -> bool {
    !matches!(family_key, "filter" | "power")
}

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
    /// Discrete choices; real values are the indices `0..labels.len()`.
    /// Presets store the index, UIs display the label.
    Stepped {
        labels: &'static [&'static str],
    },
}

impl Range {
    pub fn min(&self) -> f32 {
        match *self {
            Range::Linear { min, .. } | Range::Log { min, .. } => min,
            Range::Stepped { .. } => 0.0,
        }
    }

    pub fn max(&self) -> f32 {
        match *self {
            Range::Linear { max, .. } | Range::Log { max, .. } => max,
            Range::Stepped { labels } => (labels.len().max(1) - 1) as f32,
        }
    }

    pub fn clamp(&self, real: f32) -> f32 {
        let clamped = real.clamp(self.min(), self.max());
        match *self {
            Range::Stepped { .. } => clamped.round(),
            _ => clamped,
        }
    }

    pub fn to_real(&self, normalized: f32) -> f32 {
        let n = normalized.clamp(0.0, 1.0);
        match *self {
            Range::Linear { min, max } => min + (max - min) * n,
            Range::Log { min, max } => min * (max / min).powf(n),
            Range::Stepped { .. } => (self.max() * n).round(),
        }
    }

    pub fn to_norm(&self, real: f32) -> f32 {
        let r = self.clamp(real);
        let n = match *self {
            Range::Linear { min, max } => (r - min) / (max - min),
            Range::Log { min, max } => (r / min).ln() / (max / min).ln(),
            Range::Stepped { .. } => {
                if self.max() > 0.0 {
                    r / self.max()
                } else {
                    0.0
                }
            }
        };
        n.clamp(0.0, 1.0)
    }

    /// The label for a real value of a stepped range, `None` otherwise.
    pub fn label(&self, real: f32) -> Option<&'static str> {
        match *self {
            Range::Stepped { labels } => labels.get(self.clamp(real) as usize).copied(),
            _ => None,
        }
    }

    /// The index of a label in a stepped range (case-insensitive).
    pub fn index_of_label(&self, label: &str) -> Option<f32> {
        match *self {
            Range::Stepped { labels } => labels
                .iter()
                .position(|l| l.eq_ignore_ascii_case(label))
                .map(|i| i as f32),
            _ => None,
        }
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

/// Static description of one concrete pedal: its machine key, display name,
/// and its own parameter list. Since PRD 001 every pedal owns its params —
/// nothing is shared or inherited across pedals of the same family.
#[derive(Debug)]
pub struct EffectDesc {
    /// Machine name used in CLI/presets, e.g. `"ts9"` (or the family key for
    /// single-pedal families, e.g. `"gate"`).
    pub key: &'static str,
    pub name: &'static str,
    pub params: &'static [ParamDesc],
}

impl EffectDesc {
    pub fn param_index(&self, key: &str) -> Option<usize> {
        self.params.iter().position(|p| p.key == key)
    }
}

/// Static description of one chain-slot category: a family of selectable
/// pedals sharing one position in the chain (PRD 001). Single-pedal families
/// list exactly one descriptor whose key doubles as the family key.
#[derive(Debug)]
pub struct FamilyDesc {
    /// Machine name used in CLI/presets/MIDI, e.g. `"drive"`.
    pub key: &'static str,
    pub name: &'static str,
    /// Selectable pedals, in menu order. Append-only: presets and the v2
    /// migration reference pedals by key, plugins by id — never reorder.
    pub pedals: &'static [&'static EffectDesc],
}

impl FamilyDesc {
    /// Resolve a pedal by key or display name (case-insensitive).
    pub fn pedal_index(&self, selector: &str) -> Option<usize> {
        self.pedals.iter().position(|p| {
            p.key.eq_ignore_ascii_case(selector) || p.name.eq_ignore_ascii_case(selector)
        })
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

    #[test]
    fn stepped_range_quantizes_and_labels() {
        let r = Range::Stepped {
            labels: &["chorus", "flanger", "phaser", "tremolo"],
        };
        assert_eq!(r.max(), 3.0);
        // Quantization: real values snap to whole indices.
        assert_eq!(r.clamp(1.4), 1.0);
        assert_eq!(r.clamp(1.6), 2.0);
        assert_eq!(r.clamp(9.0), 3.0);
        // Normalized round-trips hit exact steps.
        for i in 0..4 {
            let norm = r.to_norm(i as f32);
            assert_eq!(r.to_real(norm), i as f32);
        }
        assert_eq!(r.label(2.0), Some("phaser"));
        assert_eq!(r.label(2.4), Some("phaser"));
        assert_eq!(r.index_of_label("FLANGER"), Some(1.0));
        assert_eq!(r.index_of_label("wah"), None);
        // Non-stepped ranges have no labels.
        let lin = Range::Linear { min: 0.0, max: 1.0 };
        assert_eq!(lin.label(0.5), None);
        assert_eq!(lin.index_of_label("x"), None);
    }
}
