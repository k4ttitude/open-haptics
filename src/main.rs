#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use open_haptics::device::{self, MOTOR_MAP_LEN};
use open_haptics::hidraw::{Hidraw, find_node};
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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print device info, UUID, active slot and motor map (read-only)
    Info,
    Tone,
    Run,
    Restore,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Info => info(cli.device),
        // The remaining subcommands are stubs until the later tasks implement them.
        Command::Tone | Command::Run | Command::Restore => {
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

fn info(device: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let path = match device {
        Some(path) => path,
        None => find_node(SYSFS_HIDRAW.as_ref())?,
    };
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
