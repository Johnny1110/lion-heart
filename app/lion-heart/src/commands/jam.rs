//! `jam` — play through the pedalboard with a live control REPL.
//!
//! The engine session (chain, streams, assets, presets) lives in
//! [`crate::session`], shared with the GUI; this file is only the REPL:
//! parse a command, call the session, print what came back.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::cli::JamArgs;
use crate::session::{Session, SessionOpts, list_presets};

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

pub fn run(args: JamArgs) -> Result<()> {
    let mut session = Session::start(&SessionOpts {
        input: args.io.input.clone(),
        output: args.io.output.clone(),
        sample_rate: args.io.sample_rate,
        buffer: args.io.buffer_opt(),
        in_channel: args.io.in_channel,
        gain_db: args.gain_db,
        prefill_blocks: args.prefill_blocks,
        // The REPL has no tuner, but keeping the tap installed means every
        // jam run (incl. the null-device smoke test under assert_no_alloc)
        // exercises the tap's real-time path.
        tuner_tap: true,
        midi_port: args.midi.clone(),
    })?;

    println!("{}", session.description());
    println!("{}\n", session.midi_status);

    if let Some(name) = session.initial_preset(args.preset.clone()) {
        println!("loading preset {name:?}…");
        load_preset(&mut session, &name);
        println!();
    }

    println!("chain: {}", session.chain.order_keys().join(" → "));
    print_state(&session);
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
        use std::io::BufRead;
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
        session.collect_garbage();
        for line in session.drain_midi() {
            println!("  {line}");
        }

        match line_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if !handle_line(line.trim(), &mut session) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    let snap = session.stats();
    drop(session);
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

fn print_state(session: &Session) {
    for line in session.chain.state_lines() {
        println!("  {line}");
    }
    let (nam, ir) = session.asset_names();
    println!("  assets  nam: {nam}   ir: {ir}");
}

fn load_preset(session: &mut Session, name: &str) {
    match session.load_preset(name) {
        Ok(lines) => {
            for line in lines {
                println!("  {line}");
            }
        }
        Err(e) => println!("  error: {e}"),
    }
}

/// Returns false when the session should end.
fn handle_line(line: &str, session: &mut Session) -> bool {
    let mut parts = line.split_whitespace();
    match parts.next() {
        None => {}
        Some("quit") | Some("exit") | Some("q") => return false,
        Some("help") | Some("h") | Some("?") => println!("{HELP}"),
        Some("list") | Some("l") => {
            println!("  chain: {}", session.chain.order_keys().join(" → "));
            print_state(session);
        }
        Some("meter") | Some("m") => println!("  {}", session.chain.meter_line()),
        Some("stats") => {
            let s = session.stats();
            println!(
                "  underruns {} | overruns {} | max cb {:.3} ms | errors {}",
                s.underrun_events,
                s.overrun_events,
                s.max_callback_millis(),
                s.stream_errors,
            );
        }
        Some("save") => match parts.next() {
            Some(name) if parts.next().is_none() => match session.save_preset(name) {
                Ok(msg) => println!("  {msg}"),
                Err(e) => println!("  error: {e}"),
            },
            _ => println!("usage: save <name>   (letters, digits, - and _)"),
        },
        Some("presets") => {
            let names = list_presets();
            if names.is_empty() {
                println!("  no presets yet — `save <name>` creates one");
            } else {
                for n in names {
                    println!("  {n}");
                }
            }
        }
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
                Some("nam") => match session.load_nam(Path::new(&target)) {
                    Ok(msg) => println!("  {msg}"),
                    Err(e) => println!("  error: {e}"),
                },
                Some("ir") => match session.load_ir(Path::new(&target)) {
                    Ok(msg) => println!("  {msg}"),
                    Err(e) => println!("  error: {e}"),
                },
                Some("preset") => load_preset(session, &target),
                _ => println!("usage: load nam <path> | load ir <path> | load preset <name>"),
            }
        }
        Some("unload") => match parts.next() {
            Some("nam") => {
                if session.unload_nam() {
                    println!("  nam: unloaded");
                }
            }
            Some("ir") => {
                if session.unload_ir() {
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
                // Numbers first; otherwise stepped params accept their
                // labels, e.g. `set mod.type flanger`.
                let v = match value.parse::<f32>() {
                    Ok(v) => v,
                    Err(_) => {
                        let by_label = session
                            .chain
                            .descriptors()
                            .iter()
                            .find(|d| d.key == slot)
                            .and_then(|d| d.params.iter().find(|p| p.key == param))
                            .and_then(|p| p.range.index_of_label(value));
                        match by_label {
                            Some(v) => v,
                            None => {
                                println!("  not a number or option: {value}");
                                return true;
                            }
                        }
                    }
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
