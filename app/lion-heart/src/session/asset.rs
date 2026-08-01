//! Asset path and ref helpers.

use super::*;

use std::path::Path;

use lh_core::preset::AssetRef;

/// An asset ref's file name for display, or `"-"` when unset.
pub(crate) fn asset_name(reference: &Option<AssetRef>) -> String {
    reference
        .as_ref()
        .and_then(|a| Path::new(&a.path).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".into())
}

/// Build an asset ref from a file path (canonicalize + hash).
pub(crate) fn asset_ref_for(path: &Path) -> Option<AssetRef> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match lh_assets::hash_file(&canonical) {
        Ok(sha256) => Some(AssetRef {
            path: canonical.display().to_string(),
            sha256,
        }),
        Err(e) => {
            eprintln!("warning: could not hash asset: {e}");
            None
        }
    }
}

/// Canonical parent directory as a string.
pub(crate) fn parent_dir(path: &Path) -> Option<String> {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .parent()
        .map(|p| p.display().to_string())
}

/// File name as a string.
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

impl super::Session {
    /// Loaded asset file names for display, `"-"` when empty.
    pub fn asset_names(&self) -> (String, String) {
        (asset_name(&self.nam_ref), asset_name(&self.ir_ref))
    }

    /// The cab's blend-IR file name, or `"-"` when none is loaded (ADR 015).
    pub fn ir_b_name(&self) -> String {
        asset_name(&self.ir_b_ref)
    }

    // --- assets ---

    pub fn load_nam(&mut self, path: &Path) -> Result<String, String> {
        let (asset, info) = load_nam_file(path, self.sample_rate).map_err(|e| e.to_string())?;
        let loudness = info
            .loudness_db
            .map(|l| format!("{l:.1} dB → normalized to -18 dB"))
            .unwrap_or_else(|| "unknown (no normalization)".into());
        if self.nam.install(asset).is_err() {
            return Err("install queue full, try again".into());
        }
        self.nam_ref = asset_ref_for(path);
        self.config.nam_dir = parent_dir(path);
        save_config(&self.config);
        Ok(format!(
            "nam: {} loaded ({} @ {} Hz, loudness {})",
            file_name(path),
            info.architecture,
            info.sample_rate,
            loudness,
        ))
    }

    /// (Re)decode the cab from its current primary + blend IR refs and install
    /// the combined asset in one hot-swap (ADR 015). Both files are re-read
    /// (control-thread, cheap) so whichever IRs are set ride the single swap;
    /// no primary IR clears the cab. Returns the primary IR's info for status.
    fn rebuild_cab(&mut self) -> Result<Option<lh_assets::IrInfo>, String> {
        let Some(a_ref) = self.ir_ref.clone() else {
            self.cab.clear();
            return Ok(None);
        };
        let (a, info) = lh_assets::load_ir_pair(Path::new(&a_ref.path), self.sample_rate)
            .map_err(|e| e.to_string())?;
        let b = match &self.ir_b_ref {
            Some(b_ref) => Some(
                lh_assets::load_ir_pair(Path::new(&b_ref.path), self.sample_rate)
                    .map_err(|e| e.to_string())?
                    .0,
            ),
            None => None,
        };
        if self.cab.install(Box::new(IrAsset { a, b })).is_err() {
            return Err("install queue full, try again".into());
        }
        Ok(Some(info))
    }

    /// Human-readable load note for an IR (resample/trim caveats).
    fn ir_note(path: &Path, info: &lh_assets::IrInfo) -> String {
        let mut notes = Vec::new();
        if info.resampled {
            notes.push(format!(
                "resampled {} → {} Hz",
                info.source_rate, info.engine_rate
            ));
        }
        if info.trimmed {
            notes.push(format!("trimmed to {:.0} ms", info.seconds() * 1e3));
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(", "))
        };
        format!(
            "{}, {} samples = {:.0} ms{}",
            file_name(path),
            info.used_samples,
            info.seconds() * 1e3,
            notes,
        )
    }

    /// Load the cab's **primary** IR. Any loaded blend IR is preserved and
    /// re-installed alongside it.
    pub fn load_ir(&mut self, path: &Path) -> Result<String, String> {
        let prev = self.ir_ref.clone();
        self.ir_ref = asset_ref_for(path);
        match self.rebuild_cab() {
            Ok(Some(info)) => {
                self.config.ir_dir = parent_dir(path);
                save_config(&self.config);
                Ok(format!("ir: {} loaded", Self::ir_note(path, &info)))
            }
            Ok(None) => Ok("ir cleared".into()),
            Err(e) => {
                self.ir_ref = prev; // roll back on failure — keep the old cab
                Err(e)
            }
        }
    }

    /// Load the cab's **blend** IR (a second mic/cabinet, ADR 015). Requires a
    /// primary IR already loaded; the `blend` knob crossfades between them.
    pub fn load_ir_b(&mut self, path: &Path) -> Result<String, String> {
        if self.ir_ref.is_none() {
            return Err("load a primary cab IR first, then add a blend IR".into());
        }
        let prev = self.ir_b_ref.clone();
        self.ir_b_ref = asset_ref_for(path);
        match self.rebuild_cab() {
            Ok(_) => {
                self.config.ir_dir = parent_dir(path);
                save_config(&self.config);
                Ok(format!(
                    "ir blend: {} loaded — dial the cab `blend` knob",
                    file_name(path)
                ))
            }
            Err(e) => {
                self.ir_b_ref = prev; // roll back on failure
                Err(e)
            }
        }
    }

    /// Restore the cab from a preset / carry-over: set both IR refs (resolving
    /// each against `fallback_dir`) and install them together. No primary IR
    /// clears the cab; a blend IR without a primary is dropped.
    pub(super) fn apply_cab(
        &mut self,
        ir: Option<&AssetRef>,
        ir_b: Option<&AssetRef>,
        fallback_dir: &Path,
        lines: &mut Vec<String>,
    ) {
        let Some(a_ref) = ir else {
            self.unload_ir(); // clears both refs + the cab
            return;
        };
        match lh_assets::resolve_asset(a_ref, Some(fallback_dir)) {
            Ok((a_path, warnings)) => {
                lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));
                // Set the blend ref first so the primary's load installs both
                // in one swap.
                self.ir_b_ref =
                    ir_b.and_then(|r| match lh_assets::resolve_asset(r, Some(fallback_dir)) {
                        Ok((p, w)) => {
                            lines.extend(w.into_iter().map(|w| format!("warning: {w}")));
                            asset_ref_for(&p)
                        }
                        Err(e) => {
                            lines.push(format!("error: blend ir: {e}"));
                            None
                        }
                    });
                match self.load_ir(&a_path) {
                    Ok(msg) => lines.push(msg),
                    Err(e) => lines.push(format!("error: {e}")),
                }
            }
            Err(e) => lines.push(format!("error: {e}")),
        }
    }

    /// Returns true when there was something to unload.
    pub fn unload_nam(&mut self) -> bool {
        let had = self.nam.clear();
        if had {
            self.nam_ref = None;
        }
        had
    }

    /// Unload the whole cab (both the primary and any blend IR).
    pub fn unload_ir(&mut self) -> bool {
        let had = self.cab.clear();
        self.ir_ref = None;
        self.ir_b_ref = None;
        had
    }

    /// Unload only the blend IR, leaving the primary cab in place.
    pub fn unload_ir_b(&mut self) -> bool {
        if self.ir_b_ref.is_none() {
            return false;
        }
        self.ir_b_ref = None;
        let _ = self.rebuild_cab(); // reinstall the primary alone
        true
    }

    // --- presets ---

    pub(super) fn apply_asset(
        &mut self,
        reference: Option<&AssetRef>,
        fallback_dir: &Path,
        kind: AssetKind,
        lines: &mut Vec<String>,
    ) {
        // The song is not a chain asset — it never reaches here (preset/resume
        // routing only ever passes NAM/IR), but keep the match total.
        if kind == AssetKind::Song {
            return;
        }
        match reference {
            Some(r) => match lh_assets::resolve_asset(r, Some(fallback_dir)) {
                Ok((path, warnings)) => {
                    lines.extend(warnings.into_iter().map(|w| format!("warning: {w}")));
                    let loaded = match kind {
                        AssetKind::Nam => self.load_nam(&path),
                        AssetKind::Ir => self.load_ir(&path),
                        AssetKind::IrB => self.load_ir_b(&path),
                        AssetKind::Song => unreachable!(), // guarded above
                    };
                    match loaded {
                        Ok(msg) => lines.push(msg),
                        Err(e) => lines.push(format!("error: {e}")),
                    }
                }
                Err(e) => lines.push(format!("error: {e}")),
            },
            None => {
                match kind {
                    AssetKind::Nam => self.unload_nam(),
                    AssetKind::Ir => self.unload_ir(),
                    AssetKind::IrB => self.unload_ir_b(),
                    AssetKind::Song => unreachable!(), // guarded above
                };
            }
        }
    }
}
