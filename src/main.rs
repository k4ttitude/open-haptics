#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Stream haptics to a Flydigi Apex 6 Pro")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Info,
    Tone,
    Run,
    Restore,
}

fn main() {
    // All four subcommands are stubs until the later tasks implement them.
    let _command = Cli::parse().command;
    println!("not implemented");
}
