use anyhow::Result;
use lh_io::latency::{LatencyOpts, measure};

use crate::cli::LatencyArgs;

pub fn run(args: LatencyArgs) -> Result<()> {
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
