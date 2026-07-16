use clap::{Args, Parser, Subcommand};
use lh_io::DEFAULT_SAMPLE_RATE;

#[derive(Parser)]
#[command(
    name = "lion-heart",
    version,
    about = "Lion-Heart — guitar amp & effects processor (M4: the face).\n\
             With no subcommand, opens the GUI."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// GUI options (used when no subcommand is given)
    #[command(flatten)]
    pub gui: GuiArgs,
}

#[derive(Subcommand)]
pub enum Command {
    /// List audio devices and their capabilities
    Devices,
    /// Run duplex passthrough (guitar in → guitar out)
    Run(RunArgs),
    /// Measure round-trip latency over a physical loopback cable
    Latency(LatencyArgs),
    /// Play through the pedalboard (gate → drive → amp → cab → delay) with a live REPL
    Jam(JamArgs),
}

#[derive(Args)]
pub struct GuiArgs {
    #[command(flatten)]
    pub io: IoArgs,
    /// Preset to load on start (default: the last one used)
    #[arg(long)]
    pub preset: Option<String>,
    /// MIDI input port (index or name substring; default: midi.json / first port)
    #[arg(long)]
    pub midi: Option<String>,
    /// Output gain in dB (applied with a 100 ms soft-start ramp)
    #[arg(long, default_value_t = 0.0)]
    pub gain_db: f32,
    /// Ring prefill in blocks; more absorbs jitter, each adds one buffer of latency
    #[arg(long, default_value_t = 1)]
    pub prefill_blocks: u32,
}

#[derive(Args)]
pub struct IoArgs {
    /// Input device: index or name substring (default: system input)
    #[arg(long)]
    pub input: Option<String>,
    /// Output device: index or name substring (default: system output)
    #[arg(long)]
    pub output: Option<String>,
    /// Sample rate in Hz (0 = follow the input device's default rate)
    #[arg(long, default_value_t = DEFAULT_SAMPLE_RATE)]
    pub sample_rate: u32,
    /// Requested buffer size in frames (0 = device default)
    #[arg(long, default_value_t = 64)]
    pub buffer: u32,
    /// Input channel to tap, 1-based
    #[arg(long, default_value_t = 1)]
    pub in_channel: u16,
}

impl IoArgs {
    pub fn buffer_opt(&self) -> Option<u32> {
        (self.buffer > 0).then_some(self.buffer)
    }
}

#[derive(Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub io: IoArgs,
    /// Output gain in dB (applied with a 100 ms soft-start ramp)
    #[arg(long, default_value_t = 0.0)]
    pub gain_db: f32,
    /// Stop after this many seconds (0 = run until Ctrl-C)
    #[arg(long, default_value_t = 0)]
    pub duration: u64,
    /// Ring prefill in blocks; more absorbs jitter, each adds one buffer of latency
    #[arg(long, default_value_t = 1)]
    pub prefill_blocks: u32,
}

#[derive(Args)]
pub struct JamArgs {
    #[command(flatten)]
    pub io: IoArgs,
    /// Preset to load on start (default: the last one used)
    #[arg(long)]
    pub preset: Option<String>,
    /// MIDI input port (index or name substring; default: midi.json / first port)
    #[arg(long)]
    pub midi: Option<String>,
    /// Output gain in dB (applied with a 100 ms soft-start ramp)
    #[arg(long, default_value_t = 0.0)]
    pub gain_db: f32,
    /// Stop after this many seconds (0 = run until quit/Ctrl-C)
    #[arg(long, default_value_t = 0)]
    pub duration: u64,
    /// Ring prefill in blocks; more absorbs jitter, each adds one buffer of latency
    #[arg(long, default_value_t = 1)]
    pub prefill_blocks: u32,
}

#[derive(Args)]
pub struct LatencyArgs {
    #[command(flatten)]
    pub io: IoArgs,
    /// Number of measurement trials
    #[arg(long, default_value_t = 10)]
    pub trials: u32,
    /// Gap between test bursts in milliseconds
    #[arg(long, default_value_t = 300)]
    pub interval_ms: u32,
    /// Test burst amplitude, 0.0–1.0
    #[arg(long, default_value_t = 0.5)]
    pub amplitude: f32,
    /// Also print a markdown snippet for docs/latency.md
    #[arg(long)]
    pub markdown: bool,
}
