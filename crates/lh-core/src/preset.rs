//! Preset schema (white paper §4.3): versioned JSON, real-world values keyed
//! by machine names, external assets referenced by path **and** content hash
//! so files survive moves between machines.
//!
//! Forward compatibility rules: unknown slot/pedal/param keys are skipped
//! with a warning by the applier; missing keys keep their defaults; a file
//! with a newer `schema_version` than ours is rejected instead of
//! half-loaded.
//!
//! v3 (PRD 001): a slot stores its selected pedal plus **per-pedal** param
//! maps — every pedal of the family keeps its own values, so switching
//! pedals restores each one's knobs.
//!
//! v4 (PRD 004): the delay slot became a family (digital/tape/vintage); its
//! lone v3 pedal `delay` is renamed to `digital`, whose faceplate is a
//! superset of the old one — old files sound the same (a hair brighter).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PRESET_SCHEMA_VERSION: u32 = 4;

/// Stepped index of the "classic" model in the v2 drive registry, the target
/// of the v1 migration. The v2→v3 migration then resolves indices through
/// [`DRIVE_PEDALS`].
pub const CLASSIC_DRIVE_MODEL: f32 = 2.0;

/// v2 drive model indices → v3 pedal keys, in registry order (append-only;
/// pedals past index 4 postdate v2 and are unreachable from the migration).
/// The registry lives in `lh-dsp` (which this crate cannot see); a test over
/// there pins the two together so they cannot drift.
pub const DRIVE_PEDALS: [&str; 8] = [
    "ts9",
    "bd2",
    "classic",
    "centaur",
    "evva",
    "red-charlie",
    "monster5150",
    "angry-charlie",
];

/// v2 modulation type indices → v3 pedal keys, same pinning contract.
pub const MOD_PEDALS: [&str; 4] = ["chorus", "flanger", "phaser", "tremolo"];

/// The delay family's pedal keys in registry order (PRD 004). The v3→v4
/// migration renames the old single `delay` pedal to the first of these; a
/// test over in `lh-dsp` pins this to `delay::FAMILY.pedals`.
pub const DELAY_PEDALS: [&str; 3] = ["digital", "tape", "vintage"];

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
    /// Family key ("drive", "gate", …).
    pub key: String,
    #[serde(default = "default_true")]
    pub active: bool,
    /// Selected pedal key; absent = the family's first pedal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pedal: Option<String>,
    /// Per-pedal real values: pedal key → param key → value (BTreeMap ⇒
    /// stable JSON diffs). Pedals not mentioned keep their defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pedals: BTreeMap<String, BTreeMap<String, f32>>,
    /// v1/v2 flat params, kept as migration input; empty in v3 files.
    /// Appliers treat a non-empty map as values for the selected pedal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f32>,
}

impl Default for SlotState {
    /// Matches the serde defaults (notably `active: true`), so
    /// `SlotState { key, ..Default::default() }` builds a live slot.
    fn default() -> Self {
        Self {
            key: String::new(),
            active: true,
            pedal: None,
            pedals: BTreeMap::new(),
            params: BTreeMap::new(),
        }
    }
}

impl SlotState {
    /// Convenience for tests/appliers: the value map of `pedal`.
    pub fn pedal_params(&self, pedal: &str) -> Option<&BTreeMap<String, f32>> {
        self.pedals.get(pedal)
    }
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
        }
        if preset.schema_version < 3 {
            migrate_v2_pedal_slots(&mut preset);
        }
        if preset.schema_version < 4 {
            migrate_v3_delay_pedal(&mut preset);
        }
        preset.schema_version = PRESET_SCHEMA_VERSION;
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

/// v2 → v3 (PRD 001): flat shared params become per-pedal maps.
///
/// - drive: the `model` index picks the pedal key; shared knob keys are
///   renamed onto the pedal's own faceplate (bd2/centaur/evva).
/// - mod: the `type` index picks the pedal. Tremolo lost its redundant
///   `mix` knob; v2's `dry + mix·(wet − dry)` with `wet = x·(1 − depth·…)`
///   equals v3's wet-only output at `depth' = depth × mix` exactly, so the
///   fold is audibly transparent. Absent params pin the v2 defaults — the
///   v3 tremolo defaults belong to a fresh pedal.
/// - everything else: flat params move under the family's own pedal key.
fn migrate_v2_pedal_slots(preset: &mut Preset) {
    for slot in &mut preset.chain {
        if slot.params.is_empty() && slot.pedal.is_none() && slot.pedals.is_empty() {
            continue; // nothing recorded — defaults apply either way
        }
        let old = std::mem::take(&mut slot.params);
        match slot.key.as_str() {
            "drive" => {
                let index = old.get("model").copied().unwrap_or(0.0).round() as usize;
                let pedal = DRIVE_PEDALS[index.min(DRIVE_PEDALS.len() - 1)];
                let renames: &[(&str, &str)] = match pedal {
                    "bd2" => &[("drive", "gain"), ("tone", "tone"), ("level", "level")],
                    "centaur" => &[("drive", "gain"), ("tone", "treble"), ("level", "output")],
                    "evva" => &[
                        ("drive", "gain"),
                        ("low", "low"),
                        ("mid", "mid"),
                        ("high", "high"),
                        ("level", "level"),
                    ],
                    // ts9 / classic wear the original faceplate.
                    _ => &[("drive", "drive"), ("tone", "tone"), ("level", "level")],
                };
                let mut values = BTreeMap::new();
                for (from, to) in renames {
                    if let Some(v) = old.get(*from) {
                        values.insert((*to).to_string(), *v);
                    }
                }
                slot.pedal = Some(pedal.to_string());
                if !values.is_empty() {
                    slot.pedals.insert(pedal.to_string(), values);
                }
            }
            "mod" => {
                let index = old.get("type").copied().unwrap_or(0.0).round() as usize;
                let pedal = MOD_PEDALS[index.min(MOD_PEDALS.len() - 1)];
                let mut values = BTreeMap::new();
                if pedal == "tremolo" {
                    const OLD_DEPTH: f32 = 0.5;
                    const OLD_MIX: f32 = 0.5;
                    const OLD_RATE: f32 = 0.8;
                    let depth = old.get("depth").copied().unwrap_or(OLD_DEPTH);
                    let mix = old.get("mix").copied().unwrap_or(OLD_MIX);
                    values.insert("depth".to_string(), depth * mix);
                    values.insert(
                        "rate".to_string(),
                        old.get("rate").copied().unwrap_or(OLD_RATE),
                    );
                } else {
                    // chorus/flanger/phaser keep the v2 keys, ranges, and
                    // defaults — sparse files stay sparse.
                    for key in ["rate", "depth", "feedback", "mix"] {
                        if let Some(v) = old.get(key) {
                            values.insert(key.to_string(), *v);
                        }
                    }
                }
                slot.pedal = Some(pedal.to_string());
                if !values.is_empty() {
                    slot.pedals.insert(pedal.to_string(), values);
                }
            }
            _ => {
                // Single-pedal family: the pedal key is the family key.
                if !old.is_empty() {
                    slot.pedals.insert(slot.key.clone(), old);
                }
            }
        }
    }
}

/// v3 → v4 (PRD 004): the delay slot became a multi-pedal family, so its lone
/// v3 pedal `delay` is renamed to the family's first pedal, `digital`. That
/// faceplate is a superset — `time`/`feedback`/`mix` carry over verbatim,
/// `tone`/`subdivision` take their defaults — so old files sound the same
/// (digital simply defaults to a slightly brighter tone).
fn migrate_v3_delay_pedal(preset: &mut Preset) {
    const OLD: &str = "delay";
    let new = DELAY_PEDALS[0];
    for slot in preset.chain.iter_mut().filter(|s| s.key == "delay") {
        if slot.pedal.as_deref() == Some(OLD) || slot.pedal.is_none() {
            slot.pedal = Some(new.to_string());
        }
        if let Some(values) = slot.pedals.remove(OLD) {
            slot.pedals.entry(new.to_string()).or_insert(values);
        }
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
                    pedal: Some("gate".into()),
                    pedals: BTreeMap::from([(
                        "gate".into(),
                        BTreeMap::from([("threshold".into(), -50.0), ("release".into(), 120.0)]),
                    )]),
                    params: BTreeMap::new(),
                },
                SlotState {
                    key: "drive".into(),
                    active: false,
                    pedal: Some("evva".into()),
                    pedals: BTreeMap::from([
                        ("ts9".into(), BTreeMap::from([("drive".into(), 6.0)])),
                        ("evva".into(), BTreeMap::from([("gain".into(), 4.0)])),
                    ]),
                    params: BTreeMap::new(),
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
        assert_eq!(back, p);
        assert_eq!(back.chain[1].pedal.as_deref(), Some("evva"));
        assert_eq!(back.chain[1].pedals["ts9"]["drive"], 6.0);
        assert_eq!(back.chain[1].pedals["evva"]["gain"], 4.0);
        assert!(back.assets.ir.is_none());
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let minimal = r#"{
            "schema_version": 3,
            "name": "sparse",
            "chain": [{"key": "gate"}]
        }"#;
        let p = Preset::from_json(minimal).unwrap();
        assert!(p.chain[0].active, "active defaults to true");
        assert!(p.chain[0].pedal.is_none(), "pedal defaults to the first");
        assert!(p.chain[0].pedals.is_empty());
        assert!(p.assets.nam.is_none());
    }

    #[test]
    fn rejects_newer_schema_versions() {
        let future = r#"{"schema_version": 999, "name": "x", "chain": []}"#;
        let err = Preset::from_json(future).unwrap_err();
        assert!(err.contains("newer"), "{err}");
    }

    #[test]
    fn v1_drive_params_migrate_to_classic_pedal_positions() {
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
        let slot = &p.chain[1];
        assert_eq!(slot.pedal.as_deref(), Some("classic"));
        let drive = &slot.pedals["classic"];
        assert!((drive["drive"] - 4.0).abs() < 1e-4, "{}", drive["drive"]);
        assert!((drive["tone"] - 6.695).abs() < 1e-3, "{}", drive["tone"]);
        assert!((drive["level"] - 4.217).abs() < 1e-3, "{}", drive["level"]);
        assert!(slot.params.is_empty(), "flat params consumed");
        // Untouched slots wrap under their own pedal key.
        assert_eq!(p.chain[0].pedals["gate"]["threshold"], -50.0);
    }

    #[test]
    fn v1_sparse_drive_slot_pins_the_old_defaults() {
        let v1 = r#"{"schema_version": 1, "name": "sparse", "chain": [{"key": "drive"}]}"#;
        let p = Preset::from_json(v1).unwrap();
        let slot = &p.chain[0];
        assert_eq!(slot.pedal.as_deref(), Some("classic"));
        let drive = &slot.pedals["classic"];
        assert!((drive["drive"] - 4.0).abs() < 1e-4);
        assert!((drive["tone"] - 6.695).abs() < 1e-3);
        assert!((drive["level"] - 4.217).abs() < 1e-3);
    }

    #[test]
    fn v2_drive_models_map_to_their_pedal_faceplates() {
        let case = |model: f32| {
            let json = format!(
                r#"{{"schema_version": 2, "name": "m", "chain":
                    [{{"key": "drive", "params": {{"model": {model}, "drive": 7.0,
                       "tone": 4.0, "level": 6.5, "low": 3.0, "mid": 5.5, "high": 8.0}}}}]}}"#
            );
            Preset::from_json(&json).unwrap().chain.remove(0)
        };

        let ts9 = case(0.0);
        assert_eq!(ts9.pedal.as_deref(), Some("ts9"));
        assert_eq!(ts9.pedals["ts9"]["drive"], 7.0);
        assert_eq!(ts9.pedals["ts9"]["tone"], 4.0);
        assert!(!ts9.pedals["ts9"].contains_key("low"), "no EQ knobs on ts9");

        let bd2 = case(1.0);
        assert_eq!(bd2.pedal.as_deref(), Some("bd2"));
        assert_eq!(bd2.pedals["bd2"]["gain"], 7.0, "drive renamed to gain");
        assert!(!bd2.pedals["bd2"].contains_key("drive"));

        let centaur = case(3.0);
        assert_eq!(centaur.pedals["centaur"]["gain"], 7.0);
        assert_eq!(centaur.pedals["centaur"]["treble"], 4.0);
        assert_eq!(centaur.pedals["centaur"]["output"], 6.5);

        let evva = case(4.0);
        assert_eq!(evva.pedals["evva"]["gain"], 7.0);
        assert_eq!(evva.pedals["evva"]["low"], 3.0);
        assert_eq!(evva.pedals["evva"]["high"], 8.0);
        assert!(
            !evva.pedals["evva"].contains_key("tone"),
            "evva's dead tone knob is dropped"
        );
    }

    #[test]
    fn v2_mod_types_map_to_pedals_and_tremolo_folds_mix() {
        let chorus = r#"{"schema_version": 2, "name": "c", "chain":
            [{"key": "mod", "params": {"type": 0.0, "rate": 2.0, "mix": 0.8}}]}"#;
        let p = Preset::from_json(chorus).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("chorus"));
        assert_eq!(p.chain[0].pedals["chorus"]["rate"], 2.0);
        assert_eq!(p.chain[0].pedals["chorus"]["mix"], 0.8);

        let trem = r#"{"schema_version": 2, "name": "t", "chain":
            [{"key": "mod", "params": {"type": 3.0, "depth": 0.8, "mix": 0.5}}]}"#;
        let p = Preset::from_json(trem).unwrap();
        let values = &p.chain[0].pedals["tremolo"];
        assert!((values["depth"] - 0.4).abs() < 1e-6, "depth × mix");
        assert_eq!(values["rate"], 0.8, "absent rate pins the v2 default");
        assert!(!values.contains_key("mix"));

        // Sparse tremolo: v2 defaults (depth .5 × mix .5) are pinned.
        let sparse = r#"{"schema_version": 2, "name": "s", "chain":
            [{"key": "mod", "params": {"type": 3.0}}]}"#;
        let p = Preset::from_json(sparse).unwrap();
        assert!((p.chain[0].pedals["tremolo"]["depth"] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn v3_slots_are_not_remapped() {
        let v3 = r#"{
            "schema_version": 3,
            "name": "new",
            "chain": [{"key": "drive", "pedal": "bd2",
                       "pedals": {"bd2": {"gain": 7.0}, "ts9": {"drive": 3.0}}}]
        }"#;
        let p = Preset::from_json(v3).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("bd2"));
        assert_eq!(p.chain[0].pedals["bd2"]["gain"], 7.0);
        assert_eq!(p.chain[0].pedals["ts9"]["drive"], 3.0);
    }

    #[test]
    fn v3_delay_pedal_migrates_to_digital() {
        // The v3 delay slot (single `delay` pedal) becomes the `digital`
        // voice of the new family, its shared values intact.
        let v3 = r#"{
            "schema_version": 3,
            "name": "echoes",
            "chain": [{"key": "delay", "pedal": "delay",
                       "pedals": {"delay": {"time": 420.0, "feedback": 0.5, "mix": 0.3}}}]
        }"#;
        let p = Preset::from_json(v3).unwrap();
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        let slot = &p.chain[0];
        assert_eq!(slot.pedal.as_deref(), Some("digital"));
        assert!(!slot.pedals.contains_key("delay"), "old key renamed");
        let d = &slot.pedals["digital"];
        assert_eq!(d["time"], 420.0);
        assert_eq!(d["feedback"], 0.5);
        assert_eq!(d["mix"], 0.3);
    }

    #[test]
    fn v3_sparse_delay_slot_selects_digital() {
        // A bare delay slot (no pedal, no values) still lands on digital.
        let v3 = r#"{"schema_version": 3, "name": "s", "chain": [{"key": "delay"}]}"#;
        let p = Preset::from_json(v3).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("digital"));
    }

    #[test]
    fn v4_delay_slots_are_not_remapped() {
        // Native v4: the tape/vintage voices survive untouched.
        let v4 = r#"{
            "schema_version": 4,
            "name": "new",
            "chain": [{"key": "delay", "pedal": "tape",
                       "pedals": {"tape": {"wow": 0.4}}}]
        }"#;
        let p = Preset::from_json(v4).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("tape"));
        assert_eq!(p.chain[0].pedals["tape"]["wow"], 0.4);
    }
}
