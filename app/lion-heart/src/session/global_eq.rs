//! Global EQ path and loading.

use std::path::PathBuf;

/// Path to `~/.lion-heart/global_eq.json`.
pub fn global_eq_path() -> Option<PathBuf> {
    lh_assets::app_dir().map(|d| d.join("global_eq.json"))
}

/// Read `~/.lion-heart/global_eq.json` (transparent default when absent,
/// warning on bad JSON).
pub(crate) fn load_global_eq() -> lh_core::global_eq::GlobalEqState {
    let Some(path) = global_eq_path() else {
        return lh_core::global_eq::GlobalEqState::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(json) => match lh_core::global_eq::GlobalEqState::from_json(&json) {
            Ok(state) => state,
            Err(e) => {
                eprintln!("warning: {}: {e} — using defaults", path.display());
                lh_core::global_eq::GlobalEqState::default()
            }
        },
        Err(_) => lh_core::global_eq::GlobalEqState::default(),
    }
}

impl super::Session {
    pub fn eq_state(&self) -> &lh_core::global_eq::GlobalEqState {
        self.chain.eq_state()
    }

    /// Live band update (no disk write — call [`Self::save_global_eq`] at
    /// commit points: drag release, toggles, resets).
    pub fn set_eq_band(
        &mut self,
        index: usize,
        band: lh_core::global_eq::Band,
    ) -> Result<(), String> {
        self.chain
            .set_eq_band(index, band)
            .map_err(|e| e.to_string())
    }

    pub fn set_eq_active(&mut self, enabled: bool) -> Result<(), String> {
        self.chain
            .set_eq_active(enabled)
            .map_err(|e| e.to_string())?;
        self.save_global_eq();
        Ok(())
    }

    /// Reset the global EQ to its transparent default layout.
    pub fn reset_global_eq(&mut self) -> Result<(), String> {
        self.chain
            .apply_eq_state(&lh_core::global_eq::GlobalEqState::default())
            .map_err(|e| e.to_string())?;
        self.save_global_eq();
        Ok(())
    }

    /// Persist the current EQ state to `~/.lion-heart/global_eq.json`.
    pub fn save_global_eq(&self) {
        let Some(path) = global_eq_path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, self.chain.eq_state().to_json_pretty())
        };
        if let Err(e) = write() {
            eprintln!("warning: could not save global eq: {e}");
        }
    }
}
