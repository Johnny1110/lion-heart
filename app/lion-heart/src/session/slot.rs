//! `Session` chain editing: adding and removing pedal slots, pushing slot
//! state onto the audio thread, and reclaiming retired assets.

use super::*;

impl super::Session {
    /// The audio thread never deallocates: retired assets and effects die
    /// here. Call periodically from the control loop / frame tick.
    pub fn collect_garbage(&mut self) {
        self.nam.collect_garbage();
        self.cab.collect_garbage();
        self.chain.collect_garbage();
    }

    /// Apply preset chain states **including structure** (PRD 002): the
    /// session provides the effect factory; a rebuilt amp/cab gets the
    /// session's loaded asset re-applied by the caller (`load_preset` and
    /// `resume` both re-apply assets right after).
    pub(super) fn apply_chain_states(
        &mut self,
        states: &[lh_core::preset::SlotState],
    ) -> Result<Vec<String>, String> {
        let mut rebuilt = (false, false);
        let Session {
            chain,
            nam,
            cab,
            config,
            ..
        } = &mut *self;
        let spillover = config.spillover;
        chain
            .apply_preset_chain(states, spillover, &mut |key| {
                build_family_effect(nam, cab, &mut rebuilt, key)
            })
            .map_err(|e| e.to_string())
    }

    /// Add a `family_key` instance at `position` (`None` = end). Returns
    /// user-facing lines: the new handle plus any asset reloads.
    pub fn add_slot(
        &mut self,
        family_key: &str,
        position: Option<usize>,
    ) -> Result<Vec<String>, String> {
        let Some(entry) = family_entry(family_key) else {
            let known: Vec<&str> = FAMILY_REGISTRY.iter().map(|e| e.desc.key).collect();
            return Err(format!(
                "unknown family {family_key:?} — one of: {}",
                known.join(", ")
            ));
        };
        if entry.asset.is_some() && self.chain.contains_family(family_key) {
            return Err(format!(
                "only one {family_key} per chain (it mounts the loaded asset)"
            ));
        }
        let mut rebuilt = (false, false);
        let effect = {
            let Session { nam, cab, .. } = &mut *self;
            (entry.build)(nam, cab, &mut rebuilt)
        };
        let handle = self
            .chain
            .install_slot(effect, position.unwrap_or(usize::MAX))
            .map_err(|e| e.to_string())?;
        // A freshly added looper starts empty (PRD 013 LED mirror).
        if family_key == "looper" {
            self.looper_leds.insert(handle.clone(), LooperLed::Empty);
        }
        let mut lines = vec![format!(
            "added {handle} — chain: {}",
            self.chain.order_handles().join(" → ")
        )];
        // A fresh amp/cab mounts nothing yet: re-apply the session's assets.
        let fallback = presets_dir().unwrap_or_default();
        if rebuilt.0 {
            let nam_ref = self.nam_ref.clone();
            self.apply_asset(nam_ref.as_ref(), &fallback, AssetKind::Nam, &mut lines);
        }
        if rebuilt.1 {
            let ir_ref = self.ir_ref.clone();
            self.apply_asset(ir_ref.as_ref(), &fallback, AssetKind::Ir, &mut lines);
        }
        Ok(lines)
    }

    /// Remove a slot instance by handle.
    pub fn remove_slot(&mut self, handle: &str) -> Result<String, String> {
        // A tailed slot (delay/reverb) spills — its tail rings out in a
        // spill lane rather than being cut (PRD 010) — when spillover is on.
        let spill = self.config.spillover && self.chain.slot_has_tail(handle);
        let verb = if spill {
            self.chain.spill_slot(handle).map_err(|e| e.to_string())?;
            "spilled"
        } else {
            self.chain.remove_slot(handle).map_err(|e| e.to_string())?;
            "removed"
        };
        // A spilled slot desyncs pickup like any structure change (PRD 008).
        self.midi_desync_slot(handle);
        self.looper_leds.remove(handle);
        Ok(format!(
            "{verb} {handle} — chain: {}",
            self.chain.order_handles().join(" → ")
        ))
    }
}
