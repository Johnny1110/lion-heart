use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use lh_io::passthrough::{Passthrough, PassthroughOpts};
use lh_io::stats::Snapshot;

use crate::cli::RunArgs;

pub fn run(args: RunArgs) -> Result<()> {
    let opts = PassthroughOpts {
        input: args.io.input.clone(),
        output: args.io.output.clone(),
        sample_rate: args.io.sample_rate,
        buffer: args.io.buffer_opt(),
        in_channel: args.io.in_channel(),
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
