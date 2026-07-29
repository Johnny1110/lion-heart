mod cli;
mod commands;
#[cfg(feature = "gui")]
mod gui;
mod leveling;
mod recorder;
mod render;
mod session;
mod setlist;
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
        None => {
            #[cfg(feature = "gui")]
            {
                gui::run(cli.gui)
            }
            #[cfg(not(feature = "gui"))]
            {
                eprintln!("GUI not compiled in. Build with --features gui or use a subcommand.");
                eprintln!("Subcommands: devices, run, latency, jam, render, level");
                std::process::exit(1);
            }
        }
        Some(Command::Devices) => commands::devices::run(),
        Some(Command::Run(args)) => commands::run::run(args),
        Some(Command::Latency(args)) => commands::latency::run(args),
        Some(Command::Jam(args)) => commands::jam::run(args),
        Some(Command::Render(args)) => commands::render::run(args),
        Some(Command::Level(args)) => commands::level::run(args),
    }
}
