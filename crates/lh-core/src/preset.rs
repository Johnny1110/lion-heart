//! Preset schema (white paper §4.3): versioned JSON, real-world values keyed
//! by machine names, external assets referenced by path **and** content hash
//! so files survive moves between machines.
//!
//! Forward compatibility rules: unknown slot/param keys are skipped with a
//! warning by the applier; missing keys keep their defaults; a file with a
//! newer `schema_version` than ours is rejected instead of half-loaded.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PRESET_SCHEMA_VERSION: u32 = 2;

/// Stepped index of the "classic" model in the drive registry. The registry
/// lives in `lh-dsp` (which this crate cannot see); a test over there pins
/// the two together so they cannot drift.
pub const CLASSIC_DRIVE_MODEL: f32 = 2.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub schema_version: u32,
    pub name: String,
    /// Slots in processing order; order in the file *is* the chain order.
    pub chain: Vec<SlotState>,
    #[serde(default)]
    pub assets: PresetAssets,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotState {
    pub key: String,
    #[serde(default = "default_true")]
    pub active: bool,
    /// Real-world values keyed by param key (BTreeMap ⇒ stable JSON diffs).
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PresetAssets {
    #[serde(default)]
    pub nam: Option<AssetRef>,
    #[serde(default)]
    pub ir: Option<AssetRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRef {
    pub path: String,
    /// SHA-256 of the file contents (hex), captured at save time.
    pub sha256: String,
}

fn default_true() -> bool {
    true
}

impl Preset {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("preset serialization is infallible")
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut preset: Preset = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if preset.schema_version > PRESET_SCHEMA_VERSION {
            return Err(format!(
                "preset schema v{} is newer than this build understands (v{}) — update Lion-Heart",
                preset.schema_version, PRESET_SCHEMA_VERSION
            ));
        }
        if preset.schema_version < 2 {
            migrate_v1_drive_knobs(&mut preset);
            preset.schema_version = PRESET_SCHEMA_VERSION;
        }
        Ok(preset)
    }
}

/// v1 → v2: the drive slot's params changed from real units (dB / Hz / dB)
/// to pedal-style knob positions 0..10, and the slot gained a stepped
/// `model` param. v1 files predate the model registry, so they get the
/// "classic" model and their values pass through the inverse knob laws —
/// the audible result is unchanged. Missing params are pinned to the old
/// defaults (the new defaults belong to a different model).
fn migrate_v1_drive_knobs(preset: &mut Preset) {
    use crate::{db_to_lin, drive_law};
    const OLD_DRIVE_DB: f32 = 16.0;
    const OLD_TONE_HZ: f32 = 3_200.0;
    const OLD_LEVEL_DB: f32 = -6.0;

    for slot in preset.chain.iter_mut().filter(|s| s.key == "drive") {
        let drive_db = slot.params.get("drive").copied().unwrap_or(OLD_DRIVE_DB);
        let tone_hz = slot.params.get("tone").copied().unwrap_or(OLD_TONE_HZ);
        let level_db = slot.params.get("level").copied().unwrap_or(OLD_LEVEL_DB);
        slot.params
            .insert("drive".into(), drive_law::classic_drive_pos(drive_db));
        slot.params
            .insert("tone".into(), drive_law::classic_tone_pos(tone_hz));
        slot.params
            .insert("level".into(), drive_law::level_pos(db_to_lin(level_db)));
        slot.params.insert("model".into(), CLASSIC_DRIVE_MODEL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Preset {
        Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: "lead".into(),
            chain: vec![
                SlotState {
                    key: "gate".into(),
                    active: true,
                    params: BTreeMap::from([
                        ("threshold".into(), -50.0),
                        ("release".into(), 120.0),
                    ]),
                },
                SlotState {
                    key: "drive".into(),
                    active: false,
                    params: BTreeMap::from([("model".into(), 0.0), ("drive".into(), 6.0)]),
                },
            ],
            assets: PresetAssets {
                nam: Some(AssetRef {
                    path: "/captures/plexi.nam".into(),
                    sha256: "abc123".into(),
                }),
                ir: None,
            },
        }
    }

    #[test]
    fn roundtrips_through_json() {
        let p = sample();
        let json = p.to_json_pretty();
        let back = Preset::from_json(&json).unwrap();
        assert_eq!(back.name, "lead");
        assert_eq!(back.chain.len(), 2);
        assert_eq!(back.chain[1].key, "drive");
        assert!(!back.chain[1].active);
        assert_eq!(back.chain[0].params["threshold"], -50.0);
        assert_eq!(back.assets.nam.as_ref().unwrap().sha256, "abc123");
        assert!(back.assets.ir.is_none());
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let minimal = r#"{
            "schema_version": 1,
            "name": "sparse",
            "chain": [{"key": "gate"}]
        }"#;
        let p = Preset::from_json(minimal).unwrap();
        assert!(p.chain[0].active, "active defaults to true");
        assert!(p.chain[0].params.is_empty());
        assert!(p.assets.nam.is_none());
    }

    #[test]
    fn rejects_newer_schema_versions() {
        let future = r#"{"schema_version": 999, "name": "x", "chain": []}"#;
        let err = Preset::from_json(future).unwrap_err();
        assert!(err.contains("newer"), "{err}");
    }

    #[test]
    fn v1_drive_params_migrate_to_knob_positions() {
        let v1 = r#"{
            "schema_version": 1,
            "name": "old",
            "chain": [
                {"key": "gate", "params": {"threshold": -50.0}},
                {"key": "drive", "params": {"drive": 16.0, "tone": 3200.0, "level": -6.0}}
            ]
        }"#;
        let p = Preset::from_json(v1).unwrap();
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        let drive = &p.chain[1].params;
        assert_eq!(drive["model"], CLASSIC_DRIVE_MODEL);
        assert!((drive["drive"] - 4.0).abs() < 1e-4, "{}", drive["drive"]);
        assert!((drive["tone"] - 6.695).abs() < 1e-3, "{}", drive["tone"]);
        assert!((drive["level"] - 4.217).abs() < 1e-3, "{}", drive["level"]);
        // Untouched slots pass through as-is.
        assert_eq!(p.chain[0].params["threshold"], -50.0);
    }

    #[test]
    fn v1_sparse_drive_slot_pins_the_old_defaults() {
        // A v1 file that mentions drive without params meant "old defaults"
        // (16 dB / 3200 Hz / -6 dB) — the migration must pin those, because
        // the v2 defaults belong to a different model.
        let v1 = r#"{"schema_version": 1, "name": "sparse", "chain": [{"key": "drive"}]}"#;
        let p = Preset::from_json(v1).unwrap();
        let drive = &p.chain[0].params;
        assert_eq!(drive["model"], CLASSIC_DRIVE_MODEL);
        assert!((drive["drive"] - 4.0).abs() < 1e-4);
        assert!((drive["tone"] - 6.695).abs() < 1e-3);
        assert!((drive["level"] - 4.217).abs() < 1e-3);
    }

    #[test]
    fn v2_drive_params_are_not_remapped() {
        let v2 = r#"{
            "schema_version": 2,
            "name": "new",
            "chain": [{"key": "drive", "params": {"model": 1.0, "drive": 7.0}}]
        }"#;
        let p = Preset::from_json(v2).unwrap();
        assert_eq!(p.chain[0].params["model"], 1.0);
        assert_eq!(p.chain[0].params["drive"], 7.0);
    }
}
