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
//!
//! v5 (PRD 005): the reverb slot became a twelve-machine family; its lone v4
//! pedal `reverb` is renamed to `hall`, whose faceplate is a superset of the
//! old one (`decay`/`tone`/`predelay`/`mix` keep their keys, ranges, and
//! defaults) — old files sound the same.
//!
//! v6 (PRD 009): a preset may carry up to four **snapshots** (A–D) — value
//! and bypass overlays on the one board — plus the scene that was active at
//! save time. Both fields default to empty, so a v5 file is a v6 file with
//! no scenes: it loads and sounds identical. The version still bumps so an
//! older build rejects a scene-bearing file outright instead of silently
//! dropping the scenes it cannot represent.
//!
//! v7 (ADR 015): the cab may carry a second **blend IR** (`assets.ir_b`) — a
//! second mic/cabinet. It defaults to absent, so a v6 file is a v7 file with a
//! single-mic cab: loads and sounds identical. The version bumps for the same
//! reason as v6 — an older build rejects a dual-IR preset rather than silently
//! dropping the second mic.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PRESET_SCHEMA_VERSION: u32 = 8;

/// Snapshot slot letters, in order (PRD 009). A preset stores at most this
/// many scenes; the GUI shows one chip per letter.
pub const SNAPSHOT_SLOTS: [&str; 4] = ["A", "B", "C", "D"];

/// Stepped index of the "classic" model in the v2 drive registry, the target
/// of the v1 migration. The v2→v3 migration then resolves indices through
/// [`DRIVE_PEDALS`].
pub const CLASSIC_DRIVE_MODEL: f32 = 2.0;

/// v2 drive model indices → v3 pedal keys, in registry order (append-only;
/// pedals past index 4 postdate v2 and are unreachable from the migration).
/// The registry lives in `lh-dsp` (which this crate cannot see); a test over
/// there pins the two together so they cannot drift.
pub const DRIVE_PEDALS: [&str; 12] = [
    "ts9",
    "bd2",
    "classic",
    "centaur",
    "evva",
    "red-charlie",
    "monster5150",
    "angry-charlie",
    "jan-ray",
    "fuzz-face",
    "overdrive",
    "screamer",
];

/// v2 modulation type indices → v3 pedal keys, same pinning contract.
pub const MOD_PEDALS: [&str; 4] = ["chorus", "flanger", "phaser", "tremolo"];

/// The delay family's pedal keys in registry order (PRD 004). The v3→v4
/// migration renames the old single `delay` pedal to the first of these; a
/// test over in `lh-dsp` pins this to `delay::FAMILY.pedals`.
pub const DELAY_PEDALS: [&str; 3] = ["digital", "tape", "vintage"];

/// The reverb family's pedal keys in registry order (PRD 005). `hall` leads
/// because it *is* the old M5 FDN voicing: the v4→v5 migration renames the
/// single `reverb` pedal onto it, and sparse slots (no pedal recorded)
/// default to the family's first pedal — both paths must land on the old
/// sound. A test in `lh-dsp` pins this to `reverb::FAMILY.pedals`.
pub const REVERB_PEDALS: [&str; 12] = [
    "hall",
    "room",
    "plate",
    "spring",
    "swell",
    "bloom",
    "cloud",
    "chorale",
    "shimmer",
    "magneto",
    "nonlinear",
    "reflections",
];

/// The compressor family's pedal keys in registry order (PRD 015). `vca`
/// leads because it *is* the old single `comp` pedal: the v7→v8 migration
/// renames the old `comp` pedal onto it, and sparse slots (no pedal recorded)
/// default to the family's first pedal — both paths must land on the old
/// sound. A test in `lh-dsp` pins this to `comp::FAMILY.pedals`.
pub const COMP_PEDALS: [&str; 3] = ["vca", "opto", "fet"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub schema_version: u32,
    pub name: String,
    /// Slots in processing order; order in the file *is* the chain order.
    pub chain: Vec<SlotState>,
    #[serde(default)]
    pub assets: PresetAssets,
    /// Scenes (PRD 009), keyed by letter ("A".."D"); sparse. Value+bypass
    /// overlays on `chain` — never structure. Empty in v1–v5 files.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub snapshots: BTreeMap<String, Snapshot>,
    /// The scene active at save time; re-applied on load if it still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_snapshot: Option<String>,
}

/// One scene within a preset (PRD 009): value + bypass overrides on the
/// preset's fixed board. Keyed by slot handle so a reorder keeps scenes
/// aligned to the right slots. A snapshot never changes structure or pedal
/// selection — only the selected pedal's knob values and per-slot active.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Slot handle ("drive", "drive2", …) → the slot's scene state.
    #[serde(default)]
    pub slots: BTreeMap<String, SnapshotSlot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotSlot {
    #[serde(default = "default_true")]
    pub active: bool,
    /// The selected pedal's param key → real value.
    #[serde(default)]
    pub values: BTreeMap<String, f32>,
}

impl Default for SnapshotSlot {
    fn default() -> Self {
        Self {
            active: true,
            values: BTreeMap::new(),
        }
    }
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
    /// The cab's primary IR.
    #[serde(default)]
    pub ir: Option<AssetRef>,
    /// The cab's optional blend IR — a second mic/cabinet the `blend` knob
    /// crossfades toward (ADR 015). Absent in v6 and earlier presets.
    #[serde(default)]
    pub ir_b: Option<AssetRef>,
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
        if preset.schema_version < 5 {
            migrate_v4_reverb_pedal(&mut preset);
        }
        if preset.schema_version < 8 {
            migrate_v7_comp_pedal(&mut preset);
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

/// v4 → v5 (PRD 005): the reverb slot became a multi-pedal family, so its
/// lone v4 pedal `reverb` is renamed to the family's first pedal, `hall`.
/// That faceplate is a superset — `decay`/`tone`/`predelay`/`mix` carry over
/// verbatim (same keys, same ranges), the new knobs (`mod`/`size`/`lowend`)
/// default to neutral — so old files sound the same.
fn migrate_v4_reverb_pedal(preset: &mut Preset) {
    const OLD: &str = "reverb";
    let new = REVERB_PEDALS[0];
    for slot in preset.chain.iter_mut().filter(|s| s.key == "reverb") {
        if slot.pedal.as_deref() == Some(OLD) || slot.pedal.is_none() {
            slot.pedal = Some(new.to_string());
        }
        if let Some(values) = slot.pedals.remove(OLD) {
            slot.pedals.entry(new.to_string()).or_insert(values);
        }
    }
}

/// v7 → v8 (PRD 015): the compressor slot became a multi-pedal family, so its
/// lone `comp` pedal is renamed to the family's first pedal, `vca`. That
/// faceplate is a superset — `threshold`/`ratio`/`attack`/`release`/`makeup`
/// carry over verbatim (same keys, same ranges), the new shared knobs
/// (`blend`/`sc_hpf`) default to neutral (fully-compressed / bypassed
/// sidechain) — so old files sound the same. Runs for every file below v8
/// (v5–v7 gained only `#[serde(default)]` fields, no comp change) so any
/// `comp`-keyed pedal values land on `vca` regardless of intervening versions.
fn migrate_v7_comp_pedal(preset: &mut Preset) {
    const OLD: &str = "comp";
    let new = COMP_PEDALS[0];
    for slot in preset.chain.iter_mut().filter(|s| s.key == "comp") {
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
                ir_b: None,
            },
            snapshots: BTreeMap::new(),
            active_snapshot: None,
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

    /// v6 (PRD 009): scenes round-trip, and a scene-less preset serializes
    /// without the snapshot fields (a v5 file stays clean bar the version).
    #[test]
    fn snapshots_round_trip_and_stay_optional() {
        let mut p = sample();
        p.snapshots.insert(
            "A".into(),
            Snapshot {
                slots: BTreeMap::from([(
                    "drive".into(),
                    SnapshotSlot {
                        active: true,
                        values: BTreeMap::from([("gain".into(), 8.0)]),
                    },
                )]),
            },
        );
        p.active_snapshot = Some("A".into());
        let back = Preset::from_json(&p.to_json_pretty()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.snapshots["A"].slots["drive"].values["gain"], 8.0);

        // No scenes → the fields are absent from the JSON.
        let json = sample().to_json_pretty();
        assert!(!json.contains("snapshots"), "{json}");
        assert!(!json.contains("active_snapshot"), "{json}");
    }

    /// A v5 file (no snapshot fields) upgrades to v6 with empty scenes and
    /// an otherwise identical chain — old presets are untouched.
    #[test]
    fn v5_upgrades_to_v6_without_scenes() {
        let v5 = r#"{
            "schema_version": 5,
            "name": "old",
            "chain": [{"key": "reverb", "pedal": "hall",
                       "pedals": {"hall": {"decay": 3.0}}}]
        }"#;
        let p = Preset::from_json(v5).unwrap();
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        assert!(p.snapshots.is_empty());
        assert!(p.active_snapshot.is_none());
        assert_eq!(p.chain[0].pedals["hall"]["decay"], 3.0);
    }

    /// A v6 file (single-mic cab) upgrades to v7 with no blend IR — the cab
    /// sounds identical. A v7 file round-trips its `ir_b`.
    #[test]
    fn dual_ir_field_defaults_absent_and_round_trips() {
        let v6 = r#"{
            "schema_version": 6,
            "name": "one-mic",
            "chain": [],
            "assets": {"ir": {"path": "/a.wav", "sha256": "aa"}}
        }"#;
        let p = Preset::from_json(v6).unwrap();
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        assert!(p.assets.ir.is_some());
        assert!(p.assets.ir_b.is_none(), "v6 cab has no blend IR");

        let mut dual = p.clone();
        dual.assets.ir_b = Some(AssetRef {
            path: "/b.wav".into(),
            sha256: "bb".into(),
        });
        let back = Preset::from_json(&dual.to_json_pretty()).unwrap();
        assert_eq!(back.assets.ir_b.as_ref().unwrap().path, "/b.wav");
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

    #[test]
    fn v4_reverb_pedal_migrates_to_hall() {
        // The v4 reverb slot (single `reverb` pedal) becomes the `hall`
        // voice of the new family, its values intact under the new key.
        let v4 = r#"{
            "schema_version": 4,
            "name": "wash",
            "chain": [{"key": "reverb", "pedal": "reverb",
                       "pedals": {"reverb": {"decay": 3.5, "tone": 4200.0,
                                             "predelay": 60.0, "mix": 0.4}}}]
        }"#;
        let p = Preset::from_json(v4).unwrap();
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        let slot = &p.chain[0];
        assert_eq!(slot.pedal.as_deref(), Some("hall"));
        assert!(!slot.pedals.contains_key("reverb"), "old key renamed");
        let h = &slot.pedals["hall"];
        assert_eq!(h["decay"], 3.5);
        assert_eq!(h["tone"], 4200.0);
        assert_eq!(h["predelay"], 60.0);
        assert_eq!(h["mix"], 0.4);
    }

    #[test]
    fn v4_sparse_reverb_slot_selects_hall() {
        // A bare reverb slot (no pedal, no values) still lands on hall.
        let v4 = r#"{"schema_version": 4, "name": "s", "chain": [{"key": "reverb"}]}"#;
        let p = Preset::from_json(v4).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("hall"));
    }

    #[test]
    fn v3_reverb_values_migrate_through_both_hops() {
        // v3 → v4 wraps nothing (reverb was already per-pedal keyed by the
        // family name); v4 → v5 then renames it — one old file, two hops.
        let v3 = r#"{
            "schema_version": 3,
            "name": "old wash",
            "chain": [{"key": "reverb",
                       "pedals": {"reverb": {"decay": 1.2, "mix": 0.25}}}]
        }"#;
        let p = Preset::from_json(v3).unwrap();
        let slot = &p.chain[0];
        assert_eq!(slot.pedal.as_deref(), Some("hall"));
        assert_eq!(slot.pedals["hall"]["decay"], 1.2);
        assert_eq!(slot.pedals["hall"]["mix"], 0.25);
    }

    #[test]
    fn v5_reverb_slots_are_not_remapped() {
        // Native v5: the other machines survive untouched.
        let v5 = r#"{
            "schema_version": 5,
            "name": "new",
            "chain": [{"key": "reverb", "pedal": "shimmer",
                       "pedals": {"shimmer": {"amount": 0.6},
                                  "spring": {"dwell": 0.4}}}]
        }"#;
        let p = Preset::from_json(v5).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("shimmer"));
        assert_eq!(p.chain[0].pedals["shimmer"]["amount"], 0.6);
        assert_eq!(p.chain[0].pedals["spring"]["dwell"], 0.4);
    }

    #[test]
    fn v7_comp_pedal_migrates_to_vca() {
        // The old single `comp` pedal becomes the `vca` voice of the new
        // family, its threshold/ratio/attack/release/makeup intact — so the
        // compressor sounds identical.
        let v7 = r#"{
            "schema_version": 7,
            "name": "squash",
            "chain": [{"key": "comp", "pedal": "comp",
                       "pedals": {"comp": {"threshold": -18.0, "ratio": 6.0,
                                           "attack": 8.0, "release": 200.0,
                                           "makeup": 4.0}}}]
        }"#;
        let p = Preset::from_json(v7).unwrap();
        assert_eq!(p.schema_version, PRESET_SCHEMA_VERSION);
        let slot = &p.chain[0];
        assert_eq!(slot.pedal.as_deref(), Some("vca"));
        assert!(!slot.pedals.contains_key("comp"), "old key renamed");
        let v = &slot.pedals["vca"];
        assert_eq!(v["threshold"], -18.0);
        assert_eq!(v["ratio"], 6.0);
        assert_eq!(v["attack"], 8.0);
        assert_eq!(v["release"], 200.0);
        assert_eq!(v["makeup"], 4.0);
    }

    #[test]
    fn v7_sparse_comp_slot_selects_vca() {
        // A bare comp slot (no pedal, no values) still lands on vca.
        let v7 = r#"{"schema_version": 7, "name": "s", "chain": [{"key": "comp"}]}"#;
        let p = Preset::from_json(v7).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("vca"));
    }

    #[test]
    fn old_comp_values_migrate_through_to_vca() {
        // A v3 file (comp already per-pedal keyed by the family name) survives
        // every intervening version and lands on vca with its values.
        let v3 = r#"{
            "schema_version": 3,
            "name": "old squash",
            "chain": [{"key": "comp",
                       "pedals": {"comp": {"threshold": -30.0, "ratio": 3.0}}}]
        }"#;
        let p = Preset::from_json(v3).unwrap();
        let slot = &p.chain[0];
        assert_eq!(slot.pedal.as_deref(), Some("vca"));
        assert_eq!(slot.pedals["vca"]["threshold"], -30.0);
        assert_eq!(slot.pedals["vca"]["ratio"], 3.0);
    }

    #[test]
    fn v8_comp_slots_are_not_remapped() {
        // Native v8: the opto/fet voices survive untouched.
        let v8 = r#"{
            "schema_version": 8,
            "name": "new",
            "chain": [{"key": "comp", "pedal": "opto",
                       "pedals": {"opto": {"peak_reduction": 0.6},
                                  "fet": {"ratio": 3.0}}}]
        }"#;
        let p = Preset::from_json(v8).unwrap();
        assert_eq!(p.chain[0].pedal.as_deref(), Some("opto"));
        assert_eq!(p.chain[0].pedals["opto"]["peak_reduction"], 0.6);
        assert_eq!(p.chain[0].pedals["fet"]["ratio"], 3.0);
    }
}
