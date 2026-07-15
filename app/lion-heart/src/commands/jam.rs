//! `jam` — play through the pedalboard with a live control REPL.
//!
//! M2 chain: gate → drive → NAM amp → cab IR → delay → limiter. The chain
//! runs on the audio thread; this file owns the control side: parse a
//! command, validate through the handles, push lock-free messages, and drop
//! retired assets (the audio thread never deallocates).

use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
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

use crate::cli::JamArgs;

const HELP: &str = "\
commands:
  load nam <path.nam>          load an amp capture (rate must match engine)
  load ir <path.wav>           load a cabinet impulse response
  unload nam | unload ir       remove the capture / IR
  set <slot>.<param> <value>   e.g. `set drive.drive 24`, `set amp.gain 3`
  on <slot> / off <slot>       enable / bypass a pedal (crossfaded)
  list                         pedals, values, and loaded assets
  meter                        input/output peak levels
  stats                        stream health (xruns, callback time)
  quit                         stop and exit";

struct Session {
    chain: ChainHandle,
    nam: AssetHandle<NamAsset>,
    cab: AssetHandle<IrAsset>,
    nam_name: Option<String>,
    ir_name: Option<String>,
    sample_rate: u32,
}

impl Session {
    fn print_state(&self) {
        for line in self.chain.state_lines() {
            println!("  {line}");
        }
        println!(
            "  assets  nam: {}   ir: {}",
            self.nam_name.as_deref().unwrap_or("-"),
            self.ir_name.as_deref().unwrap_or("-"),
        );
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
        nam_name: None,
        ir_name: None,
        sample_rate: runner.sample_rate,
    };

    println!("{}\n", runner.description);
    println!("chain: gate → drive → amp(NAM) → cab(IR) → delay → limiter");
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
        Some("list") | Some("l") => session.print_state(),
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
        Some("load") => {
            let kind = parts.next();
            let path: String = parts.collect::<Vec<_>>().join(" ");
            if path.is_empty() {
                println!("usage: load nam <path> | load ir <path>");
                return true;
            }
            match kind {
                Some("nam") => load_nam(session, Path::new(&path)),
                Some("ir") => load_ir(session, Path::new(&path)),
                _ => println!("usage: load nam <path> | load ir <path>"),
            }
        }
        Some("unload") => match parts.next() {
            Some("nam") => {
                if session.nam.clear() {
                    session.nam_name = None;
                    println!("  nam: unloaded");
                }
            }
            Some("ir") => {
                if session.cab.clear() {
                    session.ir_name = None;
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

fn load_nam(session: &mut Session, path: &Path) {
    match load_nam_file(path, session.sample_rate) {
        Ok((asset, info)) => {
            let loudness = info
                .loudness_db
                .map(|l| format!("{l:.1} dB → normalized to -18 dB"))
                .unwrap_or_else(|| "unknown (no normalization)".into());
            if session.nam.install(asset).is_ok() {
                session.nam_name = path.file_name().map(|s| s.to_string_lossy().into_owned());
                println!(
                    "  nam: {} loaded ({} @ {} Hz, loudness {})",
                    session.nam_name.as_deref().unwrap_or("?"),
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
                session.ir_name = path.file_name().map(|s| s.to_string_lossy().into_owned());
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
                    session.ir_name.as_deref().unwrap_or("?"),
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
