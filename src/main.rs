#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use open_haptics::capture::{Capture, PW_RECORD};
use open_haptics::device::{self, MOTOR_MAP_LEN};
use open_haptics::hidraw::{Hidraw, find_node};
use open_haptics::mapper::Mapper;
use open_haptics::profile::{builtin_profile, load_profile};
use open_haptics::session::{self, Session};
use open_haptics::{run, tone};
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

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
    /// Play a test tone on both grips (80 Hz left, 120 Hz right; writes to the pad)
    Tone {
        /// How long to play
        #[arg(long, default_value_t = 2.0)]
        seconds: f64,
    },
    /// Turn system audio into grip vibration until Ctrl+C (writes to the pad)
    Run {
        /// Mapper profile JSON; the built-in balanced profile when omitted
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Multiplies the motor drive, from 0 to 4
        #[arg(long, default_value_t = 1.0)]
        gain: f32,
    },
    /// Put back the motor modes saved by a run that did not stop cleanly (writes to the pad)
    Restore,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let writes_to_pad = match cli.command {
        Command::Restore => Some("restore"),
        Command::Tone { .. } => Some("tone"),
        Command::Run { .. } => Some("run"),
        Command::Info => None,
    };
    if let (Some(name), false) = (writes_to_pad, cli.experimental) {
        eprintln!("error: {name} writes to the pad; pass --experimental to run it");
        return ExitCode::from(2);
    }
    let result = match cli.command {
        Command::Info => info(cli.device),
        Command::Restore => restore(cli.device),
        Command::Tone { seconds } => tone(cli.device, seconds),
        Command::Run { profile, gain } => run(cli.device, profile, gain),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("open-haptics: {e}");
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

/// A flag that SIGINT, SIGTERM and SIGHUP set, so a stop restores the pad.
fn stop_flag() -> io::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    Ok(stop)
}

fn run(device: Option<PathBuf>, profile: Option<PathBuf>, gain: f32) -> Result<(), Box<dyn Error>> {
    if !(0.0..=4.0).contains(&gain) {
        return Err(format!("--gain must be a number from 0 to 4, not {gain}").into());
    }
    let profile = match profile {
        Some(path) => load_profile(&path)?,
        None => builtin_profile()?,
    };
    let stop = stop_flag()?;
    // Started before the session, so a missing pw-record leaves the pad untouched.
    let capture = Capture::spawn(PW_RECORD)?;
    let mut mapper = Mapper::new(&profile);
    let samples = capture.map(move |block| {
        let mapped = mapper.process(&block?).map_err(io::Error::other)?;
        Ok(mapped
            .into_iter()
            .map(|[l, r]| [l * gain, r * gain])
            .collect())
    });
    let session = Session::start(Hidraw::open(&node(device)?)?, &state_dir()?)?;
    Ok(run::run(session, samples, stop)?)
}

fn tone(device: Option<PathBuf>, seconds: f64) -> Result<(), Box<dyn Error>> {
    let duration = Duration::try_from_secs_f64(seconds)
        .map_err(|e| format!("--seconds must be a non-negative number: {e}"))?;
    let stop = stop_flag()?;
    let mut session = Session::start(Hidraw::open(&node(device)?)?, &state_dir()?)?;
    let played = tone::play(&mut session, duration, &stop);
    Ok(session.stop_after(played)?)
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
