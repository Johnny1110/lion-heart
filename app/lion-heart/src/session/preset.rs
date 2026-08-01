//! Preset file I/O, validation, and management helpers.

use super::*;

use std::path::{Path, PathBuf};

use lh_assets::{read_preset_order, save_preset_order};
use lh_core::preset::Preset;

/// Whether a name is a valid preset file name.
pub fn valid_preset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Read and migrate a preset file into memory (shared by load + management).
pub(crate) fn read_preset_file(path: &Path) -> Result<Preset, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Preset::from_json(&json).map_err(|e| e.to_string())
}

/// Delete `{dir}/{name}.json`, returning its path. Errors if it is absent.
pub(crate) fn delete_preset_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = dir.join(format!("{name}.json"));
    if !path.is_file() {
        return Err(format!("no preset named {name:?}"));
    }
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Copy `{src}.json` → `{new}.json` under `dir`, rewriting the stored `name`
/// to `new`; `remove_src` turns the copy into a rename. Returns the new path.
/// Backs both `Session::rename_preset` and `Session::duplicate_preset`.
pub(crate) fn copy_preset_file(
    dir: &Path,
    src: &str,
    new: &str,
    remove_src: bool,
) -> Result<PathBuf, String> {
    let from = dir.join(format!("{src}.json"));
    let to = dir.join(format!("{new}.json"));
    preset_copy_guard(src, new, from.is_file(), to.exists())?;
    let mut preset = read_preset_file(&from)?;
    preset.name = new.to_string();
    std::fs::write(&to, preset.to_json_pretty()).map_err(|e| e.to_string())?;
    if remove_src {
        std::fs::remove_file(&from).map_err(|e| e.to_string())?;
    }
    Ok(to)
}

/// Pure precondition check for a rename/duplicate: valid new name, distinct
/// names, source present, target free. Split out so it is unit-testable
/// without touching the disk.
pub(crate) fn preset_copy_guard(
    src: &str,
    new: &str,
    src_exists: bool,
    dst_exists: bool,
) -> Result<(), String> {
    if !valid_preset_name(new) {
        return Err("preset names use letters, digits, - and _ only".into());
    }
    if src == new {
        return Err("source and target names are the same".into());
    }
    if !src_exists {
        return Err(format!("no preset named {src:?}"));
    }
    if dst_exists {
        return Err(format!("a preset named {new:?} already exists"));
    }
    Ok(())
}

/// Keep `preset_order` coherent after a rename/delete/duplicate: apply `edit`
/// to the saved order and rewrite it. No-op when the user has no custom order
/// yet — everything simply stays alphabetical.
pub(crate) fn maintain_preset_order(edit: impl FnOnce(&mut Vec<String>)) {
    let mut order = read_preset_order();
    if order.is_empty() {
        return;
    }
    edit(&mut order);
    save_preset_order(&order);
}

/// A quick, human-facing digest of a preset file for the management page.
/// Even a broken file yields an `error`-tagged digest, so the page can still
/// list (and offer to delete) it.
#[derive(Debug, Clone)]
pub struct PresetInfo {
    pub name: String,
    /// "gate → drive → hall": each slot's pedal name (family key when it has
    /// none), bypassed slots parenthesized.
    pub chain: String,
    pub slots: usize,
    pub has_nam: bool,
    pub has_ir: bool,
    pub scenes: usize,
    /// Set when the file could not be read/parsed (schema too new, bad JSON).
    pub error: Option<String>,
}

/// Read `~/.lion-heart/presets/{name}.json` into a display digest.
pub fn preset_info(name: &str) -> PresetInfo {
    let mut info = PresetInfo {
        name: name.to_string(),
        chain: String::new(),
        slots: 0,
        has_nam: false,
        has_ir: false,
        scenes: 0,
        error: None,
    };
    let Some(dir) = lh_assets::presets_dir() else {
        info.error = Some("cannot determine home directory".into());
        return info;
    };
    match read_preset_file(&dir.join(format!("{name}.json"))) {
        Ok(preset) => {
            info.chain = chain_summary(&preset);
            info.slots = preset.chain.len();
            info.has_nam = preset.assets.nam.is_some();
            info.has_ir = preset.assets.ir.is_some();
            info.scenes = preset.snapshots.len();
        }
        Err(e) => info.error = Some(e),
    }
    info
}

/// A compact "gate → drive → hall" chain string. Pure — takes the parsed
/// preset — so it is testable without the disk.
pub(crate) fn chain_summary(preset: &Preset) -> String {
    if preset.chain.is_empty() {
        return "passthrough".to_string();
    }
    preset
        .chain
        .iter()
        .map(|slot| {
            let name = slot.pedal.as_deref().unwrap_or(&slot.key);
            if slot.active {
                name.to_string()
            } else {
                format!("({name})")
            }
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

impl super::Session {
    /// Save the current chain + assets. Returns the saved path message.
    pub fn save_preset(&mut self, name: &str) -> Result<String, String> {
        if !valid_preset_name(name) {
            return Err("preset names use letters, digits, - and _ only".into());
        }
        let dir = presets_dir().ok_or("cannot determine home directory")?;
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: name.to_string(),
            chain: self.chain.snapshot_chain(),
            assets: PresetAssets {
                nam: self.nam_ref.clone(),
                ir: self.ir_ref.clone(),
                ir_b: self.ir_b_ref.clone(),
            },
            snapshots: self.snapshots.clone(),
            active_snapshot: self.active_snapshot.clone(),
        };
        let path = dir.join(format!("{name}.json"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, preset.to_json_pretty()).map_err(|e| e.to_string())?;
        self.remember_preset(name);
        Ok(format!("saved {}", path.display()))
    }

    /// Load a preset by name: chain state, then both assets. Returns all
    /// user-facing lines (warnings included) in order.
    pub fn load_preset(&mut self, name: &str) -> Result<Vec<String>, String> {
        let dir = presets_dir().ok_or("cannot determine home directory")?;
        let path = dir.join(format!("{name}.json"));
        let json = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let preset = Preset::from_json(&json).map_err(|e| e.to_string())?;

        let mut lines = Vec::new();
        // The preset defines the chain structure (PRD 002): survivors keep
        // their state, missing instances are built, leftovers removed.
        let warnings = self.apply_chain_states(&preset.chain)?;
        lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));

        self.apply_asset(preset.assets.nam.as_ref(), &dir, AssetKind::Nam, &mut lines);
        self.apply_cab(
            preset.assets.ir.as_ref(),
            preset.assets.ir_b.as_ref(),
            &dir,
            &mut lines,
        );

        // Scenes come with the preset (PRD 009); apply the saved active one
        // instantly (no morph on load — it re-asserts values the baseline
        // chain already loaded).
        self.snapshots = preset.snapshots;
        self.active_snapshot = None;
        self.morph = None;
        if let Some(letter) = preset.active_snapshot {
            if self.snapshots.contains_key(&letter) {
                self.apply_snapshot(&letter, 0.0);
            }
            let count = self.snapshots.len();
            if count > 0 {
                lines.push(format!("scenes: {count} (active {letter})"));
            }
        }

        lines.push(format!(
            "preset {name:?} loaded — chain: {}",
            self.chain.order_handles().join(" → ")
        ));
        self.remember_preset(name);
        // Apply this preset's loudness trim to the output-stage master trim
        // (PRD 016) — 0 dB if none is stored.
        self.apply_preset_level(name);
        // Every param may have moved out from under a pedal: pickup-gated
        // controllers must re-engage before they speak again (PRD 008).
        self.midi_desync_all();
        // A fresh board rebuilds any looper slots empty (PRD 013 LED mirror).
        self.looper_leds.clear();
        Ok(lines)
    }

    /// Apply the stored loudness trim for `name` to the output-stage master
    /// trim (PRD 016). No stored trim → 0 dB (unity).
    fn apply_preset_level(&mut self, name: &str) {
        let trim = self.levels.trim_db(name);
        let _ = self.chain.set_master_trim_db(trim);
    }

    // --- Setlists & loudness leveling (PRD 016) --------------------------

    /// The preset a MIDI Program Change selects: the active setlist's n-th
    /// entry (clamped), else the n-th sorted preset (the zero-config
    /// cross-binary contract the plugin also honors). `None` only when there
    /// is nothing to select.
    pub fn preset_for_pc(&self, index: usize) -> Option<String> {
        match self.setlists.active_order() {
            Some(order) => setlist::preset_at_pc(order, index).map(str::to_string),
            None => list_presets().get(index).cloned(),
        }
    }

    /// The preset `delta` steps from `current` in the active navigation order
    /// (the active setlist, else the sorted directory), clamped to the ends.
    pub fn adjacent_preset(&self, current: &str, delta: isize) -> Option<String> {
        match self.setlists.active_order() {
            Some(order) => setlist::step(order, current, delta).map(str::to_string),
            None => setlist::step(&list_presets(), current, delta).map(str::to_string),
        }
    }

    /// Delete a saved preset. Clears the remembered "last preset" if it
    /// pointed here, so a deleted name is not reloaded on the next launch, and
    /// prunes it from any custom order.
    pub fn delete_preset(&mut self, name: &str) -> Result<String, String> {
        let dir = presets_dir().ok_or("cannot determine home directory")?;
        let path = delete_preset_file(&dir, name)?;
        if self.config.last_preset.as_deref() == Some(name) {
            self.config.last_preset = None;
            save_config(&self.config);
        }
        maintain_preset_order(|o| o.retain(|n| n != name));
        Ok(format!("deleted {}", path.display()))
    }

    /// Rename a preset on disk (its internal `name` field follows). Refuses
    /// to overwrite an existing target; keeps "last preset" and the custom
    /// order pointed at it (so it holds its position).
    pub fn rename_preset(&mut self, old: &str, new: &str) -> Result<String, String> {
        let dir = presets_dir().ok_or("cannot determine home directory")?;
        copy_preset_file(&dir, old, new, true)?;
        if self.config.last_preset.as_deref() == Some(old) {
            self.remember_preset(new);
        }
        maintain_preset_order(|o| {
            for n in o.iter_mut() {
                if n == old {
                    *n = new.to_string();
                }
            }
        });
        Ok(format!("renamed {old:?} → {new:?}"))
    }

    /// Copy a preset to a new name (its internal `name` follows). Refuses to
    /// overwrite; leaves the active preset unchanged and, in a custom order,
    /// drops the copy right after its source.
    pub fn duplicate_preset(&mut self, src: &str, new: &str) -> Result<String, String> {
        let dir = presets_dir().ok_or("cannot determine home directory")?;
        copy_preset_file(&dir, src, new, false)?;
        maintain_preset_order(|o| {
            if let Some(i) = o.iter().position(|n| n == src) {
                o.insert(i + 1, new.to_string());
            }
        });
        Ok(format!("copied {src:?} → {new:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use lh_core::preset::{PRESET_SCHEMA_VERSION, PresetAssets};

    // --- preset management (delete / rename / duplicate / digest) ---
    //
    // These exercise the disk helpers against an explicit temp dir, so they
    // never touch the home directory or config.json and stay parallel-safe.

    use lh_core::preset::SlotState;

    fn preset_tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lion-heart-preset-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_test_preset(dir: &Path, name: &str) {
        let preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: name.to_string(),
            chain: vec![SlotState {
                key: "gate".into(),
                ..Default::default()
            }],
            assets: PresetAssets::default(),
            snapshots: BTreeMap::new(),
            active_snapshot: None,
        };
        std::fs::write(dir.join(format!("{name}.json")), preset.to_json_pretty()).unwrap();
    }

    #[test]
    fn copy_guard_rejects_bad_inputs() {
        assert!(
            preset_copy_guard("a", "a", true, false).is_err(),
            "same name"
        );
        assert!(
            preset_copy_guard("a", "bad name", true, false).is_err(),
            "invalid new name"
        );
        assert!(
            preset_copy_guard("a", "b", false, false).is_err(),
            "missing source"
        );
        assert!(
            preset_copy_guard("a", "b", true, true).is_err(),
            "target exists"
        );
        assert!(preset_copy_guard("a", "b", true, false).is_ok());
    }

    #[test]
    fn duplicate_keeps_source_and_rewrites_internal_name() {
        let dir = preset_tmp_dir("dup");
        write_test_preset(&dir, "lead");
        let to = copy_preset_file(&dir, "lead", "lead-copy", false).unwrap();
        assert!(dir.join("lead.json").is_file(), "source kept");
        assert_eq!(
            read_preset_file(&to).unwrap().name,
            "lead-copy",
            "internal name follows the file name"
        );
    }

    #[test]
    fn rename_moves_file_and_refuses_to_clobber() {
        let dir = preset_tmp_dir("rename");
        write_test_preset(&dir, "old");
        copy_preset_file(&dir, "old", "new", true).unwrap();
        assert!(!dir.join("old.json").exists(), "source removed");
        assert_eq!(read_preset_file(&dir.join("new.json")).unwrap().name, "new");

        write_test_preset(&dir, "keep");
        assert!(
            copy_preset_file(&dir, "new", "keep", true).is_err(),
            "won't overwrite"
        );
        assert!(dir.join("new.json").is_file(), "refused rename left source");
    }

    #[test]
    fn delete_removes_file_then_reports_missing() {
        let dir = preset_tmp_dir("del");
        write_test_preset(&dir, "gone");
        assert!(delete_preset_file(&dir, "gone").is_ok());
        assert!(!dir.join("gone.json").exists());
        assert!(
            delete_preset_file(&dir, "gone").is_err(),
            "second delete errors"
        );
    }

    #[test]
    fn chain_summary_reads_pedals_and_marks_bypass() {
        let mut preset = Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: "x".into(),
            chain: vec![
                SlotState {
                    key: "gate".into(),
                    active: true,
                    ..Default::default()
                },
                SlotState {
                    key: "drive".into(),
                    active: false,
                    pedal: Some("evva".into()),
                    ..Default::default()
                },
            ],
            assets: PresetAssets::default(),
            snapshots: BTreeMap::new(),
            active_snapshot: None,
        };
        assert_eq!(chain_summary(&preset), "gate → (evva)");
        preset.chain.clear();
        assert_eq!(chain_summary(&preset), "passthrough");
    }
}
