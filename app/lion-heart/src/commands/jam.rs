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
  looper <slot> rec|undo|clear fire a looper transport (add looper first)
  learn <slot>.<param>         bind the next MIDI CC to this knob
  unlearn <slot>.<param>       clear the knob's MIDI CC binding
  snapshot <A-D>               switch scene (morphs over `morph`)
  snapshot save <A-D>          store the current scene into a slot
  morph <ms>                   scene morph time, 0–2000 ms
  tap                          tap the global tempo (two-plus in rhythm)
  tempo                        show the global tempo for synced delay/tremolo
  tempo <bpm>                  set the global tempo directly (30-300)
  spillover on|off             let delay/reverb tails ring out on switch
  metronome on|off             practice click locked to the global tempo
  click <0-100>                metronome volume (percent)
  timesig <n>                  beats per bar (accent on 1)
  countin                      (re)start the click on beat 1
  groove <name>|on|off         drum groove (rock/funk/metal/ballad) at the tempo
  groove vol <0-100>           drum volume (percent)
  fill                         arm a one-bar drum fill on the next downbeat
  song load <path>             load a backing track (WAV/MP3)
  song play|stop               start / stop the backing track
  song speed <0.25-2.0>        varispeed (pitch unchanged)
  song pitch <-12..12>         transpose in semitones
  song mix <0-100>             backing-track level (percent)
  song seek <0.0-1.0>          jump to a fraction of the track
  song loop <a> <b>            A-B loop in seconds (no args clears it)
  on <slot> / off <slot>       enable / bypass a pedal (crossfaded)
  record start|stop            record DI + wet WAVs (re-amp with `render`)
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
        if let Some(msg) = session.poll_song() {
            println!("  {msg}");
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
        Some("record") | Some("rec") => match parts.next() {
            Some("start") | Some("on") => match session.start_recording() {
                Ok((di, wet)) => {
                    println!("  recording → {}", di.display());
                    println!("             {}", wet.display());
                }
                Err(e) => println!("  error: {e}"),
            },
            Some("stop") | Some("off") => match session.stop_recording() {
                Ok(summary) => println!("  {}", summary.human()),
                Err(e) => println!("  error: {e}"),
            },
            None => match session.recording_status() {
                Some(s) => println!(
                    "  recording {:.1}s (dropped {} frames)",
                    s.elapsed.as_secs_f32(),
                    s.dropped
                ),
                None => println!("  not recording — usage: record start|stop"),
            },
            _ => println!("  usage: record start|stop"),
        },
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
        Some("looper") => {
            // `looper <slot> rec|undo|clear` fires a momentary transport pulse
            // (PRD 013). `<slot>` defaults to "looper"; reverse/half/level/mix
            // are ordinary params via `set`.
            let a = parts.next();
            let b = parts.next();
            let (slot, action) = match (a, b) {
                (Some(x), Some(y)) => (x, Some(y)),
                (Some(x @ ("rec" | "undo" | "clear")), None) => ("looper", Some(x)),
                (Some(x), None) => (x, None),
                (None, _) => ("looper", None),
            };
            match action {
                Some(act @ ("rec" | "undo" | "clear")) => match session.looper_press(slot, act) {
                    Ok(()) => println!("  {slot} {act}"),
                    Err(e) => println!("  error: {e}"),
                },
                _ => println!(
                    "usage: looper <slot> rec|undo|clear   (reverse/half/level/mix via `set`)"
                ),
            }
        }
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
        Some("tap") => {
            let msg = session.tap_tempo(None);
            if msg.is_empty() {
                println!("  tap again in rhythm to set the tempo");
            } else {
                println!("  {msg}");
            }
        }
        Some("tempo") | Some("bpm") => match parts.next() {
            Some(bpm) => match bpm.parse::<f32>() {
                Ok(v) => println!("  {}", session.set_tempo_bpm(v)),
                Err(_) => println!("  not a number: {bpm}"),
            },
            None => println!("  tempo is ♩ = {:.0} bpm", session.tempo_bpm()),
        },
        Some("spillover") => match parts.next() {
            Some("on") => println!("  {}", session.set_spillover(true)),
            Some("off") => println!("  {}", session.set_spillover(false)),
            _ => println!(
                "  spillover is {} — usage: spillover on|off",
                if session.spillover() { "on" } else { "off" }
            ),
        },
        Some("metronome") | Some("metro") => match parts.next() {
            Some("on") => println!("  {}", session.set_metronome(true)),
            Some("off") => println!("  {}", session.set_metronome(false)),
            _ => println!(
                "  metronome is {} — usage: metronome on|off",
                if session.metronome_on() { "on" } else { "off" }
            ),
        },
        Some("click") => match parts.next() {
            Some(pct) => match pct.parse::<f32>() {
                Ok(v) => println!("  {}", session.set_click_volume(v / 100.0)),
                Err(_) => println!("  not a number: {pct}"),
            },
            None => println!("  click volume is {:.0}%", session.click_volume() * 100.0),
        },
        Some("timesig") | Some("sig") => match parts.next() {
            Some(n) => match n.parse::<u32>() {
                Ok(v) => println!("  {}", session.set_beats_per_bar(v)),
                Err(_) => println!("  not a number: {n}"),
            },
            None => println!("  time signature is {}/4", session.beats_per_bar()),
        },
        Some("countin") | Some("count") => println!("  {}", session.count_in()),
        Some("groove") | Some("drums") => match parts.next() {
            Some("off") => println!("  {}", session.set_groove(false)),
            Some("on") => println!("  {}", session.set_groove(true)),
            Some("vol") | Some("volume") => match parts.next() {
                Some(pct) => match pct.parse::<f32>() {
                    Ok(v) => println!("  {}", session.set_groove_volume(v / 100.0)),
                    Err(_) => println!("  not a number: {pct}"),
                },
                None => println!("  drum volume is {:.0}%", session.groove_volume() * 100.0),
            },
            Some("fill") => println!("  {}", session.groove_fill()),
            // `groove <name>` selects the pattern and starts playing.
            Some(name) => match session.set_groove_pattern(name) {
                Ok(msg) => {
                    session.set_groove(true);
                    println!("  {msg} — playing");
                }
                Err(e) => println!("  {e}"),
            },
            None => println!(
                "  groove is {} on {:?} — usage: groove <name>|on|off|vol <n>|fill",
                if session.groove_on() { "on" } else { "off" },
                session.groove_pattern_name(),
            ),
        },
        Some("fill") => println!("  {}", session.groove_fill()),
        Some("song") => match parts.next() {
            Some("load") => match parts.next() {
                Some(path) => println!("  {}", session.load_song(std::path::Path::new(path))),
                None => println!("  usage: song load <path.wav|.mp3>"),
            },
            Some("play") => println!("  {}", session.song_play()),
            Some("stop") => println!("  {}", session.song_stop()),
            Some("speed") => match parts.next().map(|s| s.parse::<f32>()) {
                Some(Ok(v)) => println!("  {}", session.set_song_speed(v)),
                _ => println!("  usage: song speed <0.25-2.0>"),
            },
            Some("pitch") | Some("transpose") => match parts.next().map(|s| s.parse::<f32>()) {
                Some(Ok(v)) => println!("  {}", session.set_song_semitones(v)),
                _ => println!("  usage: song pitch <-12..12 semitones>"),
            },
            Some("mix") => match parts.next().map(|s| s.parse::<f32>()) {
                Some(Ok(v)) => println!("  {}", session.set_song_mix(v / 100.0)),
                _ => println!("  usage: song mix <0-100>"),
            },
            Some("seek") => match parts.next().map(|s| s.parse::<f32>()) {
                // A fraction 0..1 of the song.
                Some(Ok(v)) => {
                    session.song_seek_fraction(v);
                    println!("  seek to {:.0}%", v * 100.0);
                }
                _ => println!("  usage: song seek <0.0-1.0>"),
            },
            Some("loop") => {
                let a = parts.next().and_then(|s| s.parse::<f32>().ok());
                let b = parts.next().and_then(|s| s.parse::<f32>().ok());
                let secs = session.song_seconds().max(1e-3);
                match (a, b) {
                    // Seconds → fractions.
                    (Some(a), Some(b)) => {
                        println!("  {}", session.set_song_loop_fraction(a / secs, b / secs))
                    }
                    _ => println!("  {}", session.clear_song_loop()),
                }
            }
            _ => println!(
                "  song: {} {} — usage: song load|play|stop|speed|pitch|mix|seek|loop",
                session.song_name().unwrap_or("(none)"),
                if session.song_is_playing() {
                    "▶"
                } else {
                    "■"
                },
            ),
        },
        Some(other) => println!("  unknown command {other:?} — try `help`"),
    }
    true
}
