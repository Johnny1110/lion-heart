//! `jam` — play through the pedalboard with a live control REPL.
//!
//! Chain: gate → drive → NAM amp → cab IR → delay → limiter, reorderable at
//! runtime. The chain runs on the audio thread; this file owns the control
//! side: parse a command, validate through the handles, push lock-free
//! messages, drop retired assets, and persist presets/config to
//! `~/.lion-heart/` (last-used preset auto-loads on start).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use lh_core::preset::{AssetRef, PRESET_SCHEMA_VERSION, Preset, PresetAssets};
use lh_dsp::Effect;
use lh_dsp::cab::{CabIr, IrAsset};
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::gate::NoiseGate;
use lh_dsp::limiter::Limiter;
use lh_dsp::swap::AssetHandle;
use lh_engine::{ChainHandle, build_chain};
use lh_io::passthrough::{DuplexRunner, RunnerOpts};
use lh_nam::{NamAmp, NamAsset, load_nam_file};
use serde::{Deserialize, Serialize};

use crate::cli::JamArgs;

const HELP: &str = "\
commands:
  load nam <path.nam>          load an amp capture (rate must match engine)
  load ir <path.wav>           load a cabinet impulse response
  load preset <name>           load a saved preset (chain + assets)
  unload nam | unload ir       remove the capture / IR
  save <name>                  save chain + assets as a preset
  presets                      list saved presets
  order <slot> <slot> ...      reorder the chain (limiter stays last)
  set <slot>.<param> <value>   e.g. `set drive.drive 24`, `set amp.gain 3`
  on <slot> / off <slot>       enable / bypass a pedal (crossfaded)
  list                         pedals, values, and loaded assets
  meter                        input/output peak levels
  stats                        stream health (xruns, callback time)
  quit                         stop and exit";

#[derive(Debug, Default, Serialize, Deserialize)]
struct AppConfig {
    #[serde(default)]
    last_preset: Option<String>,
}

struct Session {
    chain: ChainHandle,
    nam: AssetHandle<NamAsset>,
    cab: AssetHandle<IrAsset>,
    nam_ref: Option<AssetRef>,
    ir_ref: Option<AssetRef>,
    sample_rate: u32,
    config: AppConfig,
}

impl Session {
    fn print_state(&self) {
        for line in self.chain.state_lines() {
            println!("  {line}");
        }
        let name = |r: &Option<AssetRef>| {
            r.as_ref()
                .and_then(|a| Path::new(&a.path).file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".into())
        };
        println!(
            "  assets  nam: {}   ir: {}",
            name(&self.nam_ref),
            name(&self.ir_ref)
        );
    }

    fn remember_preset(&mut self, name: &str) {
        self.config.last_preset = Some(name.to_string());
        save_config(&self.config);
    }
}

pub fn run(args: JamArgs) -> Result<()> {
    let (nam_amp, nam_handle) = NamAmp::new();
    let (cab, cab_handle) = CabIr::new();
    let effects: Vec<Box<dyn Effect>> = vec![
        Box::new(NoiseGate::new()),
        Box::new(Drive::new()),
        Box::new(nam_amp),
        Box::new(cab),
        Box::new(Delay::new()),
        Box::new(Limiter::new()),
    ];
    let (mut chain, chain_handle) = build_chain(effects);

    let opts = RunnerOpts {
        input: args.io.input.clone(),
        output: args.io.output.clone(),
        sample_rate: args.io.sample_rate,
        buffer: args.io.buffer_opt(),
        in_channel: args.io.in_channel,
        gain_db: args.gain_db,
        prefill_blocks: args.prefill_blocks,
    };
    let runner = DuplexRunner::start(&opts, move |info| {
        chain.prepare(info.sample_rate);
        Box::new(move |block: &mut [f32]| chain.process(block))
    })?;

    let mut session = Session {
        chain: chain_handle,
        nam: nam_handle,
        cab: cab_handle,
        nam_ref: None,
        ir_ref: None,
        sample_rate: runner.sample_rate,
        config: load_config(),
    };

    println!("{}\n", runner.description);

    // Load the requested preset, or fall back to the last one used.
    let initial = args
        .preset
        .clone()
        .or_else(|| session.config.last_preset.clone());
    if let Some(name) = initial {
        println!("loading preset {name:?}…");
        load_preset_by_name(&mut session, &name);
        println!();
    }

    println!("chain: {}", session.chain.order_keys().join(" → "));
    session.print_state();
    println!("\n{HELP}\n");

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))?;
    }

    // Blocking stdin reader feeding the control loop; drops on EOF, which
    // keeps piped input (smoke tests) working with --duration.
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    let started = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if args.duration > 0 && started.elapsed().as_secs() >= args.duration {
            break;
        }
        // The audio thread never deallocates: retired assets die here.
        session.nam.collect_garbage();
        session.cab.collect_garbage();

        match line_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if !handle_line(line.trim(), &mut session, &runner) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    let snap = runner.stats();
    drop(runner);
    println!(
        "\nsession: underruns {} ({} frames) | overruns {} ({} frames) | max cb {:.3} ms | errors {}",
        snap.underrun_events,
        snap.underrun_frames,
        snap.overrun_events,
        snap.overrun_frames,
        snap.max_callback_millis(),
        snap.stream_errors,
    );
    if !snap.has_xruns() {
        println!("verdict: clean run, no xruns");
    }
    Ok(())
}

/// Returns false when the session should end.
fn handle_line(line: &str, session: &mut Session, runner: &DuplexRunner) -> bool {
    let mut parts = line.split_whitespace();
    match parts.next() {
        None => {}
        Some("quit") | Some("exit") | Some("q") => return false,
        Some("help") | Some("h") | Some("?") => println!("{HELP}"),
        Some("list") | Some("l") => {
            println!("  chain: {}", session.chain.order_keys().join(" → "));
            session.print_state();
        }
        Some("meter") | Some("m") => println!("  {}", session.chain.meter_line()),
        Some("stats") => {
            let s = runner.stats();
            println!(
                "  underruns {} | overruns {} | max cb {:.3} ms | errors {}",
                s.underrun_events,
                s.overrun_events,
                s.max_callback_millis(),
                s.stream_errors,
            );
        }
        Some("save") => match parts.next() {
            Some(name) if parts.next().is_none() => save_preset(session, name),
            _ => println!("usage: save <name>   (letters, digits, - and _)"),
        },
        Some("presets") => list_presets(),
        Some("order") => {
            let mut keys: Vec<String> = parts.map(str::to_string).collect();
            if keys.is_empty() {
                println!("  current: {}", session.chain.order_keys().join(" → "));
                println!("  usage: order <slot> <slot> …   (limiter is appended last)");
                return true;
            }
            if !keys.iter().any(|k| k == "limiter") {
                keys.push("limiter".into());
            }
            let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            match session.chain.set_order(&refs) {
                Ok(()) => println!("  chain: {}", session.chain.order_keys().join(" → ")),
                Err(e) => println!("  error: {e}"),
            }
        }
        Some("load") => {
            let kind = parts.next();
            let target: String = parts.collect::<Vec<_>>().join(" ");
            if target.is_empty() {
                println!("usage: load nam <path> | load ir <path> | load preset <name>");
                return true;
            }
            match kind {
                Some("nam") => load_nam(session, Path::new(&target)),
                Some("ir") => load_ir(session, Path::new(&target)),
                Some("preset") => load_preset_by_name(session, &target),
                _ => println!("usage: load nam <path> | load ir <path> | load preset <name>"),
            }
        }
        Some("unload") => match parts.next() {
            Some("nam") => {
                if session.nam.clear() {
                    session.nam_ref = None;
                    println!("  nam: unloaded");
                }
            }
            Some("ir") => {
                if session.cab.clear() {
                    session.ir_ref = None;
                    println!("  ir: unloaded");
                }
            }
            _ => println!("usage: unload nam | unload ir"),
        },
        Some(toggle @ ("on" | "off")) => match parts.next() {
            Some(slot) => match session.chain.set_active(slot, toggle == "on") {
                Ok(()) => println!("  {slot}: {toggle}"),
                Err(e) => println!("  error: {e}"),
            },
            None => println!("usage: {toggle} <slot>"),
        },
        Some("set") => match (parts.next(), parts.next()) {
            (Some(path), Some(value)) => {
                let Some((slot, param)) = path.split_once('.') else {
                    println!("usage: set <slot>.<param> <value>");
                    return true;
                };
                let Ok(v) = value.parse::<f32>() else {
                    println!("  not a number: {value}");
                    return true;
                };
                match session.chain.set_param(slot, param, v) {
                    Ok(applied) => {
                        println!("  {slot}.{param} = {:.2} {}", applied.real, applied.unit)
                    }
                    Err(e) => println!("  error: {e}"),
                }
            }
            _ => println!("usage: set <slot>.<param> <value>"),
        },
        Some(other) => println!("  unknown command {other:?} — try `help`"),
    }
    true
}

// --- assets ---

fn asset_ref_for(path: &Path) -> Option<AssetRef> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match lh_assets::hash_file(&canonical) {
        Ok(sha256) => Some(AssetRef {
            path: canonical.display().to_string(),
            sha256,
        }),
        Err(e) => {
            println!("  warning: could not hash asset: {e}");
            None
        }
    }
}

fn load_nam(session: &mut Session, path: &Path) {
    match load_nam_file(path, session.sample_rate) {
        Ok((asset, info)) => {
            let loudness = info
                .loudness_db
                .map(|l| format!("{l:.1} dB → normalized to -18 dB"))
                .unwrap_or_else(|| "unknown (no normalization)".into());
            if session.nam.install(asset).is_ok() {
                session.nam_ref = asset_ref_for(path);
                println!(
                    "  nam: {} loaded ({} @ {} Hz, loudness {})",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    info.architecture,
                    info.sample_rate,
                    loudness,
                );
            } else {
                println!("  error: install queue full, try again");
            }
        }
        Err(e) => println!("  error: {e}"),
    }
}

fn load_ir(session: &mut Session, path: &Path) {
    match lh_assets::load_ir(path, session.sample_rate) {
        Ok((asset, info)) => {
            if session.cab.install(asset).is_ok() {
                session.ir_ref = asset_ref_for(path);
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
                println!(
                    "  ir: {} loaded, {} samples = {:.0} ms{}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    info.used_samples,
                    info.seconds() * 1e3,
                    notes,
                );
            } else {
                println!("  error: install queue full, try again");
            }
        }
        Err(e) => println!("  error: {e}"),
    }
}

// --- presets & config on disk ---

fn app_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".lion-heart"))
}

fn presets_dir() -> Option<PathBuf> {
    app_dir().map(|d| d.join("presets"))
}

fn load_config() -> AppConfig {
    app_dir()
        .map(|d| d.join("config.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(config: &AppConfig) {
    let Some(dir) = app_dir() else { return };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("config.json"),
            serde_json::to_string_pretty(config).expect("config serializes"),
        )
    };
    if let Err(e) = write() {
        println!("  warning: could not save config: {e}");
    }
}

fn valid_preset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn save_preset(session: &mut Session, name: &str) {
    if !valid_preset_name(name) {
        println!("  error: preset names use letters, digits, - and _ only");
        return;
    }
    let Some(dir) = presets_dir() else {
        println!("  error: cannot determine $HOME");
        return;
    };
    let preset = Preset {
        schema_version: PRESET_SCHEMA_VERSION,
        name: name.to_string(),
        chain: session.chain.snapshot_chain(),
        assets: PresetAssets {
            nam: session.nam_ref.clone(),
            ir: session.ir_ref.clone(),
        },
    };
    let path = dir.join(format!("{name}.json"));
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, preset.to_json_pretty())
    };
    match write() {
        Ok(()) => {
            println!("  saved {}", path.display());
            session.remember_preset(name);
        }
        Err(e) => println!("  error: {e}"),
    }
}

fn list_presets() {
    let Some(dir) = presets_dir() else {
        println!("  error: cannot determine $HOME");
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("  no presets yet — `save <name>` creates one");
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json"))
                .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .flatten()
        })
        .collect();
    names.sort();
    if names.is_empty() {
        println!("  no presets yet — `save <name>` creates one");
    } else {
        for n in names {
            println!("  {n}");
        }
    }
}

fn load_preset_by_name(session: &mut Session, name: &str) {
    let Some(dir) = presets_dir() else {
        println!("  error: cannot determine $HOME");
        return;
    };
    let path = dir.join(format!("{name}.json"));
    let json = match std::fs::read_to_string(&path) {
        Ok(j) => j,
        Err(e) => {
            println!("  error: cannot read {}: {e}", path.display());
            return;
        }
    };
    let preset = match Preset::from_json(&json) {
        Ok(p) => p,
        Err(e) => {
            println!("  error: {e}");
            return;
        }
    };

    match session.chain.apply_preset_chain(&preset.chain) {
        Ok(warnings) => {
            for w in warnings {
                println!("  warning: {w}");
            }
        }
        Err(e) => {
            println!("  error: {e}");
            return;
        }
    }

    apply_asset(session, preset.assets.nam.as_ref(), &dir, AssetKind::Nam);
    apply_asset(session, preset.assets.ir.as_ref(), &dir, AssetKind::Ir);

    println!(
        "  preset {name:?} loaded — chain: {}",
        session.chain.order_keys().join(" → ")
    );
    session.remember_preset(name);
}

enum AssetKind {
    Nam,
    Ir,
}

fn apply_asset(
    session: &mut Session,
    reference: Option<&AssetRef>,
    fallback_dir: &Path,
    kind: AssetKind,
) {
    match reference {
        Some(r) => match lh_assets::resolve_asset(r, Some(fallback_dir)) {
            Ok((path, warnings)) => {
                for w in warnings {
                    println!("  warning: {w}");
                }
                match kind {
                    AssetKind::Nam => load_nam(session, &path),
                    AssetKind::Ir => load_ir(session, &path),
                }
            }
            Err(e) => println!("  error: {e}"),
        },
        None => match kind {
            AssetKind::Nam => {
                if session.nam.clear() {
                    session.nam_ref = None;
                }
            }
            AssetKind::Ir => {
                if session.cab.clear() {
                    session.ir_ref = None;
                }
            }
        },
    }
}
