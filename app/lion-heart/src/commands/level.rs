//! `level` — offline preset loudness leveling (PRD 016): render a reference DI
//! through one or every preset, measure integrated LUFS (BS.1770), and write a
//! per-preset master trim toward a target so switching presets live stops
//! jumping in volume. Reuses [`crate::render`] (the exact live chain) and
//! [`crate::leveling`]; pure and device-free.

use anyhow::{Context, Result};
use lh_assets::wav;
use lh_core::preset::Preset;

use crate::cli::LevelArgs;
use crate::leveling::{Levels, measure, reference_di, trim_for};
use crate::render::ENGINE_RATE;
use crate::session::presets_dir;

pub fn run(args: LevelArgs) -> Result<()> {
    let dir = presets_dir().context("cannot determine home directory for presets")?;

    // The reference DI: a user file (must be at the engine rate) or the
    // built-in synthesized yardstick.
    let di = match &args.reference {
        Some(path) => {
            let di = wav::read(path)
                .with_context(|| format!("cannot read reference DI {}", path.display()))?;
            if di.sample_rate != ENGINE_RATE {
                anyhow::bail!(
                    "reference DI is {} Hz but the engine runs at {ENGINE_RATE} Hz — resample it first",
                    di.sample_rate
                );
            }
            di
        }
        None => reference_di(),
    };

    let names: Vec<String> = if args.all {
        lh_assets::list_presets()
    } else {
        vec![
            args.preset
                .clone()
                .expect("clap requires --preset without --all"),
        ]
    };
    if names.is_empty() {
        println!("no presets found in {}", dir.display());
        return Ok(());
    }

    let mut levels = Levels::load();
    levels.target_lufs = args.target;
    println!("target {:.1} LUFS\n", args.target);

    let mut measured = 0;
    for name in &names {
        let path = dir.join(format!("{name}.json"));
        let preset = match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|json| Preset::from_json(&json))
        {
            Ok(p) => p,
            Err(e) => {
                println!("  {name}: skipped — {e}");
                continue;
            }
        };
        match measure(&preset, &di, Some(&dir)) {
            Ok((lufs, warnings)) => {
                for w in &warnings {
                    println!("  {name}: warning: {w}");
                }
                let trim = trim_for(lufs, args.target);
                if lufs.is_finite() {
                    println!("  {name:<24} {lufs:>7.1} LUFS   trim {trim:+5.1} dB");
                } else {
                    println!("  {name:<24}   (silent — no trim)");
                }
                if !args.dry_run {
                    levels.trims.insert(name.clone(), trim);
                }
                measured += 1;
            }
            Err(e) => println!("  {name}: skipped — {e}"),
        }
    }

    if args.dry_run {
        println!("\ndry run — {measured} preset(s) measured, levels.json untouched");
    } else {
        levels.save();
        let where_ = Levels::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "levels.json".into());
        println!("\nwrote {measured} trim(s) to {where_}");
    }
    Ok(())
}
