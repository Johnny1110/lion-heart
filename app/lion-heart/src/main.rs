mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};

// In debug builds, any allocation inside an `assert_no_alloc` section (the
// audio callback) aborts loudly — CLAUDE.md real-time rule 8. Release builds
// keep the plain system allocator.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Devices => commands::devices::run(),
        Command::Run(args) => commands::run::run(args),
        Command::Latency(args) => commands::latency::run(args),
        Command::Jam(args) => commands::jam::run(args),
    }
}
