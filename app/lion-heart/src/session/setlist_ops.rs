//! `Session` setlist operations. The setlist *data model* lives in
//! [`crate::setlist`]; this module is the session-facing behaviour over it,
//! named `setlist_ops` so the two never shadow each other.

use super::*;

impl super::Session {
    /// The setlists model (GUI/REPL read).
    pub fn setlists(&self) -> &Setlists {
        &self.setlists
    }

    /// The active setlist name plus the 1-based position of `current` within
    /// it and the list length, for a live "setlist · 3/12" readout. `None`
    /// when no setlist is active.
    pub fn setlist_position(&self, current: &str) -> Option<(String, usize, usize)> {
        let name = self.setlists.active.clone()?;
        let order = self.setlists.active_order()?;
        let pos = setlist::position(order, current)
            .map(|i| i + 1)
            .unwrap_or(0);
        Some((name, pos, order.len()))
    }

    /// Activate a setlist by name (must exist), or `None` to fall back to the
    /// sorted directory. Persists.
    pub fn set_active_setlist(&mut self, name: Option<&str>) -> Result<String, String> {
        match name {
            Some(n) => {
                if !self.setlists.lists.contains_key(n) {
                    return Err(format!("no setlist named {n:?}"));
                }
                self.setlists.active = Some(n.to_string());
                self.setlists.save();
                Ok(format!("setlist {n:?} active"))
            }
            None => {
                self.setlists.active = None;
                self.setlists.save();
                Ok("setlist off — sorted directory".into())
            }
        }
    }

    /// Create an empty setlist `name`. Persists. Errors if it already exists.
    pub fn setlist_create(&mut self, name: &str) -> Result<String, String> {
        if name.trim().is_empty() {
            return Err("setlist name required".into());
        }
        if self.setlists.lists.contains_key(name) {
            return Err(format!("setlist {name:?} already exists"));
        }
        self.setlists.lists.insert(name.to_string(), Vec::new());
        self.setlists.save();
        Ok(format!("created setlist {name:?}"))
    }

    /// Append `preset` to setlist `list` (created if new). Persists.
    pub fn setlist_add(&mut self, list: &str, preset: &str) -> Result<String, String> {
        if list.is_empty() {
            return Err("setlist name required".into());
        }
        let entry = self.setlists.lists.entry(list.to_string()).or_default();
        entry.push(preset.to_string());
        let n = entry.len();
        self.setlists.save();
        Ok(format!("added {preset:?} to {list:?} (#{n})"))
    }

    /// Delete setlist `list` (deactivating it if active). Persists.
    pub fn setlist_delete(&mut self, list: &str) -> Result<String, String> {
        if self.setlists.lists.remove(list).is_none() {
            return Err(format!("no setlist named {list:?}"));
        }
        if self.setlists.active.as_deref() == Some(list) {
            self.setlists.active = None;
        }
        self.setlists.save();
        Ok(format!("removed setlist {list:?}"))
    }

    /// Move the entry at `index` within setlist `list` by `delta` (reorder).
    /// Persists. Clamped to the list bounds.
    pub fn setlist_move(&mut self, list: &str, index: usize, delta: isize) -> Result<(), String> {
        let entries = self
            .setlists
            .lists
            .get_mut(list)
            .ok_or_else(|| format!("no setlist named {list:?}"))?;
        if index >= entries.len() {
            return Err("index out of range".into());
        }
        let target = (index as isize + delta).clamp(0, entries.len() as isize - 1) as usize;
        if target != index {
            let item = entries.remove(index);
            entries.insert(target, item);
            self.setlists.save();
        }
        Ok(())
    }

    /// Remove the entry at `index` from setlist `list`. Persists.
    pub fn setlist_remove_at(&mut self, list: &str, index: usize) -> Result<(), String> {
        let entries = self
            .setlists
            .lists
            .get_mut(list)
            .ok_or_else(|| format!("no setlist named {list:?}"))?;
        if index >= entries.len() {
            return Err("index out of range".into());
        }
        entries.remove(index);
        self.setlists.save();
        Ok(())
    }
}
