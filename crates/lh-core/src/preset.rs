//! Preset schema (white paper §4.3): versioned JSON, real-world values keyed
//! by machine names, external assets referenced by path **and** content hash
//! so files survive moves between machines.
//!
//! Forward compatibility rules: unknown slot/param keys are skipped with a
//! warning by the applier; missing keys keep their defaults; a file with a
//! newer `schema_version` than ours is rejected instead of half-loaded.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PRESET_SCHEMA_VERSION: u32 = 1;

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
        let preset: Preset = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if preset.schema_version > PRESET_SCHEMA_VERSION {
            return Err(format!(
                "preset schema v{} is newer than this build understands (v{}) — update Lion-Heart",
                preset.schema_version, PRESET_SCHEMA_VERSION
            ));
        }
        Ok(preset)
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
                    params: BTreeMap::from([("drive".into(), 24.0)]),
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
}
