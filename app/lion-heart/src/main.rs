use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use lh_io::latency::{LatencyOpts, measure};
use lh_io::passthrough::{Passthrough, PassthroughOpts};
use lh_io::stats::Snapshot;
use lh_io::{DEFAULT_SAMPLE_RATE, devices};

#[derive(Parser)]
#[command(
    name = "lion-heart",
    version,
    about = "Lion-Heart — guitar amp & effects processor (M0: audio I/O foundation)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List audio devices and their capabilities
    Devices,
    /// Run duplex passthrough (guitar in → guitar out)
    Run(RunArgs),
    /// Measure round-trip latency over a physical loopback cable
    Latency(LatencyArgs),
}

#[derive(Args)]
struct IoArgs {
    /// Input device: index or name substring (default: system input)
    #[arg(long)]
    input: Option<String>,
    /// Output device: index or name substring (default: system output)
    #[arg(long)]
    output: Option<String>,
    /// Sample rate in Hz (0 = follow the input device's default rate)
    #[arg(long, default_value_t = DEFAULT_SAMPLE_RATE)]
    sample_rate: u32,
    /// Requested buffer size in frames (0 = device default)
    #[arg(long, default_value_t = 64)]
    buffer: u32,
    /// Input channel to tap, 1-based
    #[arg(long, default_value_t = 1)]
    in_channel: u16,
}

impl IoArgs {
    fn buffer_opt(&self) -> Option<u32> {
        (self.buffer > 0).then_some(self.buffer)
    }
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    io: IoArgs,
    /// Output gain in dB (applied with a 100 ms soft-start ramp)
    #[arg(long, default_value_t = 0.0)]
    gain_db: f32,
    /// Stop after this many seconds (0 = run until Ctrl-C)
    #[arg(long, default_value_t = 0)]
    duration: u64,
    /// Ring prefill in blocks; more absorbs jitter, each adds one buffer of latency
    #[arg(long, default_value_t = 1)]
    prefill_blocks: u32,
}

#[derive(Args)]
struct LatencyArgs {
    #[command(flatten)]
    io: IoArgs,
    /// Number of measurement trials
    #[arg(long, default_value_t = 10)]
    trials: u32,
    /// Gap between test bursts in milliseconds
    #[arg(long, default_value_t = 300)]
    interval_ms: u32,
    /// Test burst amplitude, 0.0–1.0
    #[arg(long, default_value_t = 0.5)]
    amplitude: f32,
    /// Also print a markdown snippet for docs/latency.md
    #[arg(long)]
    markdown: bool,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Devices => cmd_devices(),
        Command::Run(args) => cmd_run(args),
        Command::Latency(args) => cmd_latency(args),
    }
}

fn cmd_devices() -> Result<()> {
    println!("Audio devices (host: {}):", devices::host_name());
    for dev in devices::enumerate()? {
        let mut tags = Vec::new();
        if dev.is_default_input {
            tags.push("default in");
        }
        if dev.is_default_output {
            tags.push("default out");
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", tags.join(", "))
        };
        println!("  [{}] {}{}", dev.index, dev.name, tags);
        for (label, port) in [("in ", &dev.input), ("out", &dev.output)] {
            if let Some(p) = port {
                let rates = if p.min_rate == p.max_rate {
                    format!("{} Hz", p.default_rate)
                } else {
                    format!("{} Hz ({}–{})", p.default_rate, p.min_rate, p.max_rate)
                };
                let buffer = p
                    .buffer_range
                    .map(|(lo, hi)| format!(", buffer {lo}–{hi}"))
                    .unwrap_or_default();
                println!(
                    "        {label}: {} ch {} @ {rates}{buffer}",
                    p.channels, p.sample_format
                );
            }
        }
    }
    Ok(())
}

fn cmd_run(args: RunArgs) -> Result<()> {
    let opts = PassthroughOpts {
        input: args.io.input.clone(),
        output: args.io.output.clone(),
        sample_rate: args.io.sample_rate,
        buffer: args.io.buffer_opt(),
        in_channel: args.io.in_channel,
        gain_db: args.gain_db,
        prefill_blocks: args.prefill_blocks,
    };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))?;
    }

    let passthrough = Passthrough::start(&opts)?;
    println!("{}", passthrough.description);
    println!("\npassthrough running — Ctrl-C to stop\n");

    let started = Instant::now();
    let mut last = passthrough.stats();
    let mut last_print = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
        if args.duration > 0 && started.elapsed().as_secs() >= args.duration {
            break;
        }
        let snap = passthrough.stats();
        let xruns_moved = snap.underrun_events != last.underrun_events
            || snap.overrun_events != last.overrun_events
            || snap.stream_errors != last.stream_errors;
        if xruns_moved || last_print.elapsed() >= Duration::from_secs(5) {
            print_status(started.elapsed(), &snap);
            last_print = Instant::now();
        }
        last = snap;
    }

    let final_snap = passthrough.stats();
    drop(passthrough);
    println!("\nfinal report");
    print_status(started.elapsed(), &final_snap);
    if final_snap.has_xruns() {
        println!(
            "verdict: XRUNS DETECTED — try a larger --buffer or --prefill-blocks, \
             and close other audio apps"
        );
    } else {
        println!("verdict: clean run, no xruns");
    }
    Ok(())
}

fn print_status(elapsed: Duration, s: &Snapshot) {
    println!(
        "[{:>5}s] frames in/out {}/{} | underruns {} ({} frames) | overruns {} ({} frames) | max cb {:.3} ms | errors {}",
        elapsed.as_secs(),
        s.in_frames,
        s.out_frames,
        s.underrun_events,
        s.underrun_frames,
        s.overrun_events,
        s.overrun_frames,
        s.max_callback_millis(),
        s.stream_errors,
    );
}

fn cmd_latency(args: LatencyArgs) -> Result<()> {
    let opts = LatencyOpts {
        input: args.io.input.clone(),
        output: args.io.output.clone(),
        sample_rate: args.io.sample_rate,
        buffer: args.io.buffer_opt(),
        in_channel: args.io.in_channel,
        trials: args.trials,
        interval_ms: args.interval_ms,
        amplitude: args.amplitude,
    };

    println!(
        "measuring round-trip latency: {} trials, {} ms apart",
        args.trials, args.interval_ms
    );
    println!("connect interface output → input with a cable (or enable loopback mode)\n");

    let report = measure(&opts, &mut |n, ms| {
        println!("  trial {n:>2}: {ms:>7.2} ms");
    })?;

    println!("\n{}", report.description);
    println!(
        "\nRTL median {:.2} ms  (min {:.2} / max {:.2}, {} trials, {} missed)",
        report.median_ms,
        report.min_ms,
        report.max_ms,
        report.trials_ms.len(),
        report.missed,
    );
    let fmt_actual = |n: Option<u32>| n.map(|v| v.to_string()).unwrap_or_else(|| "?".into());
    println!(
        "actual buffer in/out: {}/{} frames | noise floor {:.4} (threshold {:.4}) | stream errors {}",
        fmt_actual(report.actual_buffer.0),
        fmt_actual(report.actual_buffer.1),
        report.noise_floor,
        report.threshold,
        report.xruns.stream_errors
    );

    if args.markdown {
        println!("\n--- markdown for docs/latency.md ---\n");
        println!("{}", report.to_markdown());
    }
    Ok(())
}
