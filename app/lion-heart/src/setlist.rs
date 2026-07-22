//! Setlists (PRD 016): named, ordered lists of presets for the stage. When one
//! is **active**, prev/next, the footswitch, and MIDI Program Change walk the
//! setlist's order instead of the sorted preset directory. App-global
//! environment, persisted to `~/.lion-heart/setlists.json` — never in a preset,
//! and absent from the plugin (the host's song/scene mechanism is that answer).
//!
//! The MIDI PC contract is preserved: with no active setlist, PC *n* still
//! selects the *n*-th sorted preset (the cross-binary contract the plugin also
//! honors). Activating a setlist is a **session-side override** of that walk;
//! the plugin, which has no setlist concept, is unaffected.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lh_assets::app_dir;
use serde::{Deserialize, Serialize};

/// `~/.lion-heart/setlists.json` contents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Setlists {
    /// The active setlist's name. `None` (or a name with no matching, non-empty
    /// list) means "fall back to the sorted preset directory".
    #[serde(default)]
    pub active: Option<String>,
    /// Named setlists: name → ordered preset names.
    #[serde(default)]
    pub lists: BTreeMap<String, Vec<String>>,
}

impl Setlists {
    pub fn path() -> Option<PathBuf> {
        app_dir().map(|d| d.join("setlists.json"))
    }

    /// Read `setlists.json` (empty default when absent, warning on bad JSON).
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                eprintln!("warning: {}: {e} — ignoring setlists", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(dir) = app_dir() else { return };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("setlists.json"),
                serde_json::to_string_pretty(self).expect("setlists serialize"),
            )
        };
        if let Err(e) = write() {
            eprintln!("warning: could not save setlists: {e}");
        }
    }

    /// The active setlist's preset order, if a named, non-empty list is active.
    pub fn active_order(&self) -> Option<&[String]> {
        let name = self.active.as_ref()?;
        self.lists
            .get(name)
            .map(Vec::as_slice)
            .filter(|o| !o.is_empty())
    }

    /// Is a usable setlist active?
    pub fn is_active(&self) -> bool {
        self.active_order().is_some()
    }
}

/// The preset for MIDI Program Change `pc` (0-based) within `order`: PC *n* →
/// the *n*-th entry, clamped to the last (the existing zero-config contract,
/// now walking the setlist). `None` only if `order` is empty.
pub fn preset_at_pc(order: &[String], pc: usize) -> Option<&str> {
    if order.is_empty() {
        return None;
    }
    Some(order[pc.min(order.len() - 1)].as_str())
}

/// Index of `current` within `order`, if present.
pub fn position(order: &[String], current: &str) -> Option<usize> {
    order.iter().position(|p| p == current)
}

/// Step `delta` (±1…) from `current` within `order`, clamped to the ends. If
/// `current` is not in the list, a forward step lands on the first entry and a
/// backward step on the last. `None` only if `order` is empty.
pub fn step<'a>(order: &'a [String], current: &str, delta: isize) -> Option<&'a str> {
    if order.is_empty() {
        return None;
    }
    let last = order.len() as isize - 1;
    let next = match position(order, current) {
        Some(i) => (i as isize + delta).clamp(0, last),
        None if delta >= 0 => 0,
        None => last,
    };
    Some(order[next as usize].as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn active_order_needs_a_named_nonempty_list() {
        let mut s = Setlists::default();
        assert!(!s.is_active());
        s.lists.insert("gig".into(), list(&["a", "b"]));
        assert!(!s.is_active(), "not active until selected");
        s.active = Some("gig".into());
        assert_eq!(
            s.active_order(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
        s.active = Some("missing".into());
        assert!(!s.is_active(), "unknown name falls back");
        s.active = Some("gig".into());
        s.lists.insert("gig".into(), Vec::new());
        assert!(!s.is_active(), "empty list falls back");
    }

    #[test]
    fn pc_clamps_to_the_list() {
        let order = list(&["intro", "verse", "solo"]);
        assert_eq!(preset_at_pc(&order, 0), Some("intro"));
        assert_eq!(preset_at_pc(&order, 2), Some("solo"));
        assert_eq!(
            preset_at_pc(&order, 99),
            Some("solo"),
            "clamped, not wrapped"
        );
        assert_eq!(preset_at_pc(&[], 0), None);
    }

    #[test]
    fn step_walks_and_clamps() {
        let order = list(&["a", "b", "c"]);
        assert_eq!(step(&order, "a", 1), Some("b"));
        assert_eq!(step(&order, "c", 1), Some("c"), "clamp at the end");
        assert_eq!(step(&order, "a", -1), Some("a"), "clamp at the start");
        assert_eq!(step(&order, "b", -1), Some("a"));
        // Not in the list: forward → first, backward → last.
        assert_eq!(step(&order, "zz", 1), Some("a"));
        assert_eq!(step(&order, "zz", -1), Some("c"));
    }

    #[test]
    fn round_trips_through_json() {
        let mut s = Setlists::default();
        s.lists.insert("set-a".into(), list(&["one", "two"]));
        s.active = Some("set-a".into());
        let json = serde_json::to_string(&s).unwrap();
        let back: Setlists = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn empty_json_is_inactive() {
        let back: Setlists = serde_json::from_str("{}").unwrap();
        assert!(!back.is_active());
    }
}
