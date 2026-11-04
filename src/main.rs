#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use open_haptics::device::{self, MOTOR_MAP_LEN};
use open_haptics::hidraw::{Hidraw, find_node};
use open_haptics::session;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";

#[derive(Parser)]
#[command(version, about = "Stream haptics to a Flydigi Apex 6 Pro")]
struct Cli {
    /// hidraw node of the Apex 6 vendor interface; found automatically when omitted
    #[arg(long, global = true)]
    device: Option<PathBuf>,
    /// Allow commands that write to the pad
    #[arg(long, global = true)]
    experimental: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print device info, UUID, active slot and motor map (read-only)
    Info,
    Tone,
    Run,
    /// Put back the motor modes saved by a run that did not stop cleanly (writes to the pad)
    Restore,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if matches!(cli.command, Command::Restore) && !cli.experimental {
        eprintln!("error: restore writes to the pad; pass --experimental to run it");
        return ExitCode::from(2);
    }
    let result = match cli.command {
        Command::Info => info(cli.device),
        Command::Restore => restore(cli.device),
        // The remaining subcommands are stubs until the later tasks implement them.
        Command::Tone | Command::Run => {
            println!("not implemented");
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn node(device: Option<PathBuf>) -> Result<PathBuf, Box<dyn Error>> {
    match device {
        Some(path) => Ok(path),
        None => Ok(find_node(SYSFS_HIDRAW.as_ref())?),
    }
}

/// `$XDG_STATE_HOME/open-haptics`, or `~/.local/state/open-haptics` when that is unset or not
/// absolute (XDG base directory spec). `Session` creates it on first save.
fn state_dir() -> Result<PathBuf, Box<dyn Error>> {
    let base = match std::env::var_os("XDG_STATE_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => {
            let home = std::env::var_os("HOME")
                .ok_or("neither XDG_STATE_HOME nor HOME is set, so there is no state directory")?;
            PathBuf::from(home).join(".local/state")
        }
    };
    Ok(base.join("open-haptics"))
}

fn restore(device: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let path = node(device)?;
    let mut pad = Hidraw::open(&path)?;
    session::restore_from_file(&mut pad, &state_dir()?)?;
    println!("restored the saved motor modes and deleted the saved map");
    Ok(())
}

fn info(device: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let path = node(device)?;
    let mut pad = Hidraw::open(&path)?;
    let info = device::read_info(&mut pad)?;
    let uuid = device::read_uuid(&mut pad)?;
    let motor_map = device::read_motor_map(&mut pad)?;
    println!("node: {}", path.display());
    println!("device id: {}", info.device_id);
    println!("connection mode: {}", info.connection_mode);
    println!("firmware (main): {:#06x}", info.fw_main_version);
    println!("features: {}", info.feature_names().join(", "));
    println!("grips present (featureHaptic): {}", info.feature_haptic());
    println!("uuid: {uuid}");
    println!("slot: {}", motor_map.slot);
    let hex: Vec<String> = motor_map.map.iter().map(|b| format!("{b:02x}")).collect();
    println!("motor map ({MOTOR_MAP_LEN} bytes): {}", hex.join(" "));
    Ok(())
}
