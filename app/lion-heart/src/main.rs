mod cli;
mod commands;
mod gui;
mod recorder;
mod render;
mod session;
mod song_loader;

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
    let cli = Cli::parse();
    match cli.command {
        None => gui::run(cli.gui),
        Some(Command::Devices) => commands::devices::run(),
        Some(Command::Run(args)) => commands::run::run(args),
        Some(Command::Latency(args)) => commands::latency::run(args),
        Some(Command::Jam(args)) => commands::jam::run(args),
        Some(Command::Render(args)) => commands::render::run(args),
    }
}
