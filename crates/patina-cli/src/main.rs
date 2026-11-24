//! `patina`: the command-line adapter over `patina-core`.
#![forbid(unsafe_code)]

use clap::Parser;

/// Identity-driven personal task system.
#[derive(Debug, Parser)]
#[command(name = "patina", version, about)]
struct Cli {}

fn main() {
    // No subcommands yet: parsing handles `--help` and `--version`.
    let _cli = Cli::parse();
}
