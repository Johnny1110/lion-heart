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
  load ir <path.wav>           load a cabinet impulse response (primary mic)
  load ir_b <path.wav>         load a blend IR (2nd mic; cab `blend` mixes A⇄B)
  load preset <name>           load a saved preset (chain + assets)
  unload nam|ir|ir_b           remove the capture / cab IR / blend IR
  save <name>                  save chain + assets as a preset
  presets                      list saved presets
  delete <name>                delete a saved preset
  rename <old> <new>           rename a saved preset
  copy <src> <new>             duplicate a preset under a new name
  add <family> [pos]           insert a slot (e.g. `add drive`, `add comp 0`)
  remove <slot>                remove a slot instance (e.g. `remove drive2`)
  order <slot> <slot> ...      reorder the chain (all handles, new order)
  pedal <slot> <name>          switch the slot's pedal (e.g. `pedal drive ts9`)
  set <slot>.<param> <value>   e.g. `set drive2.drive 6`, `set drive.pedal evva`
  learn <slot>.<param>         bind the next MIDI CC to this knob
  unlearn <slot>.<param>       clear the knob's MIDI CC binding
  snapshot <A-D>               switch scene (morphs over `morph`)
  snapshot save <A-D>          store the current scene into a slot
  morph <ms>                   scene morph time, 0–2000 ms
  tempo <bpm>                  global tempo for synced delay/tremolo (`sync`)
  spillover on|off             let delay/reverb tails ring out on switch
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
        in_channel: args.io.in_channel(),
        gain_db: args.gain_db,
        prefill_blocks: args.prefill_blocks,
        // The REPL has no tuner, but keeping the tap installed means every
        // jam run (incl. the null-device smoke test under assert_no_alloc)
        // exercises the tap's real-time path.
        tuner_tap: true,
        spectrum_tap: false,
        midi_port: args.midi.clone(),
    })?;

    println!("{}", session.description());
    println!("{}\n", session.midi_status);

    if let Some(name) = session.initial_preset(args.preset.clone()) {
        println!("loading preset {name:?}…");
        load_preset(&mut session, &name);
        println!();
    }

    println!("chain: {}", session.chain.order_handles().join(" → "));
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
        session.tick_morph(Instant::now());
        session.tick_tempo();
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
            println!("  chain: {}", session.chain.order_handles().join(" → "));
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
        Some("delete") | Some("rm") => match parts.next() {
            Some(name) if parts.next().is_none() => match session.delete_preset(name) {
                Ok(msg) => println!("  {msg}"),
                Err(e) => println!("  error: {e}"),
            },
            _ => println!("usage: delete <name>"),
        },
        Some("rename") => match (parts.next(), parts.next()) {
            (Some(old), Some(new)) if parts.next().is_none() => {
                match session.rename_preset(old, new) {
                    Ok(msg) => println!("  {msg}"),
                    Err(e) => println!("  error: {e}"),
                }
            }
            _ => println!("usage: rename <old> <new>"),
        },
        Some("copy") => match (parts.next(), parts.next()) {
            (Some(src), Some(new)) if parts.next().is_none() => {
                match session.duplicate_preset(src, new) {
                    Ok(msg) => println!("  {msg}"),
                    Err(e) => println!("  error: {e}"),
                }
            }
            _ => println!("usage: copy <src> <new>"),
        },
        Some("order") => {
            let keys: Vec<String> = parts.map(str::to_string).collect();
            if keys.is_empty() {
                println!("  current: {}", session.chain.order_handles().join(" → "));
                println!("  usage: order <slot> <slot> …   (every handle, once)");
                return true;
            }
            let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            match session.chain.set_order(&refs) {
                Ok(()) => println!("  chain: {}", session.chain.order_handles().join(" → ")),
                Err(e) => println!("  error: {e}"),
            }
        }
        Some("add") => match parts.next() {
            Some(family) => {
                let position = parts.next().and_then(|p| p.parse::<usize>().ok());
                match session.add_slot(family, position) {
                    Ok(lines) => {
                        for line in lines {
                            println!("  {line}");
                        }
                    }
                    Err(e) => println!("  error: {e}"),
                }
            }
            None => println!("usage: add <family> [position]"),
        },
        Some("remove") => match parts.next() {
            Some(handle) => match session.remove_slot(handle) {
                Ok(msg) => println!("  {msg}"),
                Err(e) => println!("  error: {e}"),
            },
            None => println!("usage: remove <slot>"),
        },
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
                Some("ir_b") | Some("irb") => match session.load_ir_b(Path::new(&target)) {
                    Ok(msg) => println!("  {msg}"),
                    Err(e) => println!("  error: {e}"),
                },
                Some("preset") => load_preset(session, &target),
                _ => println!(
                    "usage: load nam <path> | load ir <path> | load ir_b <path> | load preset <name>"
                ),
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
            Some("ir_b") | Some("irb") => {
                if session.unload_ir_b() {
                    println!("  ir blend: unloaded");
                }
            }
            _ => println!("usage: unload nam | unload ir | unload ir_b"),
        },
        Some(toggle @ ("on" | "off")) => match parts.next() {
            Some(slot) => match session.chain.set_active(slot, toggle == "on") {
                Ok(()) => println!("  {slot}: {toggle}"),
                Err(e) => println!("  error: {e}"),
            },
            None => println!("usage: {toggle} <slot>"),
        },
        Some("pedal") => match parts.next() {
            Some(slot) => {
                let name: String = parts.collect::<Vec<_>>().join(" ");
                if name.is_empty() {
                    println!("usage: pedal <slot> <name>");
                } else {
                    match session.chain.select_pedal(slot, &name) {
                        Ok(pedal) => {
                            session.midi_desync_slot(slot);
                            println!("  {slot}: {pedal}");
                        }
                        Err(e) => println!("  error: {e}"),
                    }
                }
            }
            None => println!("usage: pedal <slot> <name>"),
        },
        Some("set") => match parts.next() {
            Some(path) => {
                let Some((slot, param)) = path.split_once('.') else {
                    println!("usage: set <slot>.<param> <value>");
                    return true;
                };
                // Join the rest so multi-word values work ("blues driver").
                let value: String = parts.collect::<Vec<_>>().join(" ");
                if value.is_empty() {
                    println!("usage: set <slot>.<param> <value>");
                    return true;
                }
                // `slot.pedal` (and the pre-v3 aliases) takes a pedal
                // key/name/index; everything else is numeric or a stepped
                // label.
                if lh_engine::is_pedal_selector(param) {
                    match session.chain.select_pedal(slot, &value) {
                        Ok(pedal) => {
                            session.midi_desync_slot(slot);
                            println!("  {slot}.pedal = {pedal}");
                        }
                        Err(e) => println!("  error: {e}"),
                    }
                    return true;
                }
                let v = match value.parse::<f32>() {
                    Ok(v) => v,
                    Err(_) => {
                        let by_label = session
                            .chain
                            .param_desc(slot, param)
                            .and_then(|p| p.range.index_of_label(&value));
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
                        // The knob moved out from under a pickup-gated pedal.
                        session.midi_desync_param(slot, param);
                        println!("  {slot}.{param} = {:.2} {}", applied.real, applied.unit)
                    }
                    Err(e) => println!("  error: {e}"),
                }
            }
            None => println!("usage: set <slot>.<param> <value>"),
        },
        Some("learn") => match parts.next().and_then(|p| p.split_once('.')) {
            Some((slot, param)) => match session.arm_midi_learn(slot, param) {
                Ok(msg) => println!("  {msg}"),
                Err(e) => println!("  error: {e}"),
            },
            None => println!("usage: learn <slot>.<param>"),
        },
        Some("unlearn") => match parts.next().and_then(|p| p.split_once('.')) {
            Some((slot, param)) => match session.clear_cc_binding(slot, param) {
                Ok(msg) => println!("  {msg}"),
                Err(e) => println!("  error: {e}"),
            },
            None => println!("usage: unlearn <slot>.<param>"),
        },
        Some("snapshot") | Some("snap") => {
            let arg = parts.next();
            let result = match arg {
                Some("save") | Some("store") => match parts.next() {
                    Some(letter) => session.store_snapshot(letter),
                    None => Err("usage: snapshot save <A-D>".into()),
                },
                Some(letter) => session.switch_snapshot(letter),
                None => Err("usage: snapshot <A-D> | snapshot save <A-D>".into()),
            };
            match result {
                Ok(msg) => println!("  {msg}"),
                Err(e) => println!("  error: {e}"),
            }
        }
        Some("morph") => match parts.next() {
            Some(ms) => match ms.parse::<u32>() {
                Ok(v) => println!("  {}", session.set_morph_ms(v)),
                Err(_) => println!("  not a number: {ms}"),
            },
            None => println!("  morph time is {} ms", session.morph_ms()),
        },
        Some("spillover") => match parts.next() {
            Some("on") => println!("  {}", session.set_spillover(true)),
            Some("off") => println!("  {}", session.set_spillover(false)),
            _ => println!(
                "  spillover is {} — usage: spillover on|off",
                if session.spillover() { "on" } else { "off" }
            ),
        },
        Some("tempo") | Some("bpm") => match parts.next() {
            Some(bpm) => match bpm.parse::<f32>() {
                Ok(v) => {
                    println!("  {}", session.set_tempo_bpm(v));
                    session.tick_tempo();
                }
                Err(_) => println!("  not a number: {bpm}"),
            },
            None => println!("  tempo is ♩ = {:.0} bpm", session.tempo_bpm()),
        },
        Some(other) => println!("  unknown command {other:?} — try `help`"),
    }
    true
}
