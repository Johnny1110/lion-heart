//! `jam` — play through the M1 pedalboard with a live control REPL.
//!
//! The chain runs on the audio thread; this file owns the control side:
//! parse a command line, validate through `ChainHandle`, push a lock-free
//! message. No audio state lives here.

use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use lh_dsp::Effect;
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::gate::NoiseGate;
use lh_engine::{ChainHandle, build_chain};
use lh_io::passthrough::{DuplexRunner, RunnerOpts};

use crate::cli::JamArgs;

const HELP: &str = "\
commands:
  set <slot>.<param> <value>   e.g. `set drive.drive 24`, `set delay.time 500`
  on <slot> / off <slot>       enable / bypass a pedal (crossfaded)
  list                         show pedals and current values
  meter                        input/output peak levels
  stats                        stream health (xruns, callback time)
  quit                         stop and exit";

pub fn run(args: JamArgs) -> Result<()> {
    let effects: Vec<Box<dyn Effect>> = vec![
        Box::new(NoiseGate::new()),
        Box::new(Drive::new()),
        Box::new(Delay::new()),
    ];
    let (mut chain, mut handle) = build_chain(effects);

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

    println!("{}\n", runner.description);
    println!("pedalboard: gate → drive → delay   (amp/cab land in M2)");
    for line in handle.state_lines() {
        println!("  {line}");
    }
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
        match line_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if !handle_line(line.trim(), &mut handle, &runner) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // stdin closed; keep playing until Ctrl-C or --duration.
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
fn handle_line(line: &str, handle: &mut ChainHandle, runner: &DuplexRunner) -> bool {
    let mut parts = line.split_whitespace();
    match parts.next() {
        None => {}
        Some("quit") | Some("exit") | Some("q") => return false,
        Some("help") | Some("h") | Some("?") => println!("{HELP}"),
        Some("list") | Some("l") => {
            for l in handle.state_lines() {
                println!("  {l}");
            }
        }
        Some("meter") | Some("m") => println!("  {}", handle.meter_line()),
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
        Some(toggle @ ("on" | "off")) => match parts.next() {
            Some(slot) => match handle.set_active(slot, toggle == "on") {
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
                match handle.set_param(slot, param, v) {
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
