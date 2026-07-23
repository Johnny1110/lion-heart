//! `render` — offline re-amp (PRD 014): read a DI WAV, process it through a
//! saved preset, write the wet result. Pure, device-free, CI-testable — the
//! heavy lifting is in [`crate::render`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lh_assets::wav::{self, WavBits};
use lh_core::preset::Preset;

use crate::cli::RenderArgs;
use crate::render;
use crate::session::presets_dir;

pub fn run(args: RenderArgs) -> Result<()> {
    // Load the preset by name from ~/.lion-heart/presets.
    let dir = presets_dir().context("cannot determine home directory for presets")?;
    let preset_path = dir.join(format!("{}.json", args.preset));
    let json = std::fs::read_to_string(&preset_path)
        .with_context(|| format!("cannot read preset {}", preset_path.display()))?;
    let preset = Preset::from_json(&json).map_err(|e| anyhow::anyhow!("preset: {e}"))?;

    // Read the DI.
    let di =
        wav::read(&args.di).with_context(|| format!("cannot read DI {}", args.di.display()))?;
    println!(
        "DI {} — {:.1}s, {} ch, {} Hz",
        args.di.display(),
        di.frames() as f32 / di.sample_rate.max(1) as f32,
        di.channels,
        di.sample_rate,
    );

    // Render (reuses the live chain, offline).
    let rendered =
        render::render(&preset, &di, Some(&dir), args.tail).map_err(|e| anyhow::anyhow!("{e}"))?;
    for w in &rendered.warnings {
        println!("  warning: {w}");
    }

    let out = args
        .output
        .unwrap_or_else(|| default_out(&args.di, &args.preset));
    wav::write(
        &out,
        &rendered.samples,
        2,
        render::ENGINE_RATE,
        WavBits::Float32,
    )
    .with_context(|| format!("cannot write {}", out.display()))?;
    println!(
        "rendered {:?} → {} ({:.1}s +{:.1}s tail)",
        args.preset,
        out.display(),
        di.frames() as f32 / render::ENGINE_RATE as f32,
        args.tail,
    );
    Ok(())
}

/// `<di-stem>-<preset>.wav`, next to the DI.
fn default_out(di: &Path, preset: &str) -> PathBuf {
    let stem = di
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "render".into());
    di.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}-{preset}.wav"))
}
