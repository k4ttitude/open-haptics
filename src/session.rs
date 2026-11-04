//! The grip session: the only code that changes the pad's motor modes. `start` saves the pad's
//! motor map to a file before its first `0x53` write, and `stop` puts every target the start
//! sequence touched back from that map, then deletes the file. Sequences follow the vault note
//! "Apex 6 haptics protocol", items 5 and 6, grip-only path.
use crate::device::{self, InfoError, MOTOR_MAP_LEN, MotorMapError};
use crate::hidraw::{AskError, Transport};
use crate::protocol::{
    self, AREA_REALTIME, CMD_MODE, CONTROL_START, CONTROL_STOP, GRIPS_OFF, GRIPS_STREAM,
    ProtocolError, TRIGGERS_OFF,
};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// RAM 6 layout version the engine accepts (note, item 4, 0x14028ed29).
const LAYOUT_VERSION: u8 = 0x02;
/// RAM 6 layout 2 entries in restore order: target, offset of its mode byte, and how many param
/// bytes follow for each mode; other modes reject the layout (note, item 4, 0x14028e680,
/// 0x14028e7aa; restore order from item 6 step 7, 0x140284109).
const MAP_ENTRIES: [(u8, usize, &[usize]); 4] = [
    (0x00, 1, &[0, 10, 3]),
    (0x01, 12, &[0, 10, 3]),
    (0x10, 23, &[0, 1]),
    (0x11, 25, &[0, 1]),
];

#[derive(Debug)]
pub enum SessionError {
    NotLinearGrips,
    UnknownMapLayout,
    SavedMapExists(PathBuf),
    NoSavedMap(PathBuf),
    Info(InfoError),
    MotorMap(MotorMapError),
    Ask(AskError),
    Io(io::Error),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLinearGrips => write!(
                f,
                "the pad does not report linear (LRA) grip motors (featureHaptic clear); \
                 open-haptics drives only those"
            ),
            Self::UnknownMapLayout => write!(
                f,
                "the pad's motor map has a layout this tool cannot restore, so it will not \
                 change the motor modes"
            ),
            Self::SavedMapExists(path) => write!(
                f,
                "a saved motor map from an earlier run is still at {}; that run did not \
                 restore the pad. run open-haptics restore --experimental first",
                path.display()
            ),
            Self::NoSavedMap(path) => write!(
                f,
                "no saved motor map for this pad ({} does not exist); nothing to restore",
                path.display()
            ),
            Self::Info(e) => write!(f, "reading the pad's info failed: {e}"),
            Self::MotorMap(e) => write!(f, "reading the pad's motor map failed: {e}"),
            Self::Ask(e) => e.fmt(f),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<InfoError> for SessionError {
    fn from(e: InfoError) -> Self {
        Self::Info(e)
    }
}

impl From<MotorMapError> for SessionError {
    fn from(e: MotorMapError) -> Self {
        Self::MotorMap(e)
    }
}

impl From<AskError> for SessionError {
    fn from(e: AskError) -> Self {
        Self::Ask(e)
    }
}

impl From<ProtocolError> for SessionError {
    fn from(e: ProtocolError) -> Self {
        Self::Ask(AskError::Protocol(e))
    }
}

impl From<io::Error> for SessionError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The saved map for the pad with this UUID. One file per pad, so a restore only reaches the pad
/// it was saved from.
pub fn map_path(state_dir: &Path, uuid: &str) -> PathBuf {
    state_dir.join(format!("motor-map-{uuid}.bin"))
}

pub struct Session<T: Transport> {
    t: T,
    map_path: PathBuf,
    restore: Vec<Vec<u8>>,
    /// Set before the first `0x53` write; cleared when `stop` runs.
    dirty: bool,
}

impl<T: Transport> Session<T> {
    /// Reads and saves the pad's motor map, then puts both grips in streaming mode.
    pub fn start(mut t: T, state_dir: &Path) -> Result<Self, SessionError> {
        if !device::read_info(&mut t)?.feature_haptic() {
            return Err(SessionError::NotLinearGrips);
        }
        let map_path = map_path(state_dir, &device::read_uuid(&mut t)?);
        // A leftover file is the only copy of the map from a run that died; never overwrite it.
        if map_path.try_exists()? {
            return Err(SessionError::SavedMapExists(map_path));
        }
        let map = device::read_motor_map(&mut t)?.map;
        let restore = restore_payloads(&map)?;
        save_map(&map_path, &map)?;
        let mut session = Session {
            t,
            map_path,
            restore,
            dirty: true,
        };
        if let Err(e) = session.enable() {
            if let Err(stop_error) = session.stop() {
                session.report_stop_failure(&stop_error);
            }
            return Err(e);
        }
        Ok(session)
    }

    /// Sends one `0x57` packet: the first eight samples of each grip.
    pub fn send(&mut self, left: &[f32], right: &[f32]) -> Result<(), SessionError> {
        self.t.write(&protocol::motor_packet(left, right))?;
        Ok(())
    }

    /// Restores the pad and deletes the saved map. Runs once; later calls do nothing. On an
    /// error the map file stays for `open-haptics restore --experimental`.
    pub fn stop(&mut self) -> Result<(), SessionError> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        restore_pad(&mut self.t, &self.restore)?;
        fs::remove_file(&self.map_path)?;
        Ok(())
    }

    /// Grip-only start: triggers off, grips to streaming, then `0xFC` codes 1 and 3 (note,
    /// item 5). Like the engine, it reads no reply to any of them (note, item 1, 0x14027bb80).
    fn enable(&mut self) -> Result<(), SessionError> {
        write_mode(&mut self.t, &TRIGGERS_OFF)?;
        write_mode(&mut self.t, &GRIPS_STREAM)?;
        for code in CONTROL_START {
            self.t.write(&protocol::control_frame(code))?;
        }
        Ok(())
    }

    fn report_stop_failure(&self, e: &SessionError) {
        eprintln!(
            "error: restoring the pad's motor modes failed: {e}. The saved map stays at {}; \
             run open-haptics restore --experimental",
            self.map_path.display()
        );
    }
}

impl<T: Transport> Drop for Session<T> {
    fn drop(&mut self) {
        if let Err(e) = self.stop() {
            self.report_stop_failure(&e);
        }
    }
}

/// Restores the pad of an earlier run that died, from its saved map, then deletes the file.
pub fn restore_from_file<T: Transport>(t: &mut T, state_dir: &Path) -> Result<(), SessionError> {
    let path = map_path(state_dir, &device::read_uuid(t)?);
    let map = match fs::read(&path) {
        Ok(map) => map,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(SessionError::NoSavedMap(path));
        }
        Err(e) => return Err(e.into()),
    };
    restore_pad(t, &restore_payloads(&map)?)?;
    fs::remove_file(&path)?;
    Ok(())
}

/// The stop sequence: `0xFC` code 2, triggers off, grips off (note, item 6 steps 2 to 4), then
/// the saved mode of targets 00, 01, 10 and 11 (step 7). None of these waits for a reply, as in
/// the engine (0x14027e790, 0x140284191).
fn restore_pad<T: Transport>(t: &mut T, restore: &[Vec<u8>]) -> Result<(), SessionError> {
    t.write(&protocol::control_frame(CONTROL_STOP))?;
    write_mode(t, &TRIGGERS_OFF)?;
    write_mode(t, &GRIPS_OFF)?;
    for payload in restore {
        write_mode(t, payload)?;
    }
    Ok(())
}

fn write_mode<T: Transport>(t: &mut T, payload: &[u8]) -> Result<(), SessionError> {
    t.write(&protocol::frame(CMD_MODE, payload)?)?;
    Ok(())
}

/// Builds the `01 target mode params` restore write for each RAM 6 entry, refusing any layout
/// the engine would reject (note, item 4).
fn restore_payloads(map: &[u8]) -> Result<Vec<Vec<u8>>, SessionError> {
    if map.len() != MOTOR_MAP_LEN || map[0] != LAYOUT_VERSION {
        return Err(SessionError::UnknownMapLayout);
    }
    let mut payloads = Vec::new();
    for (target, offset, kept) in MAP_ENTRIES {
        let mode = map[offset];
        let params = *kept
            .get(usize::from(mode))
            .ok_or(SessionError::UnknownMapLayout)?;
        let mut payload = vec![AREA_REALTIME, target];
        payload.extend_from_slice(&map[offset..offset + 1 + params]);
        payloads.push(payload);
    }
    Ok(payloads)
}

/// Writes the map to a temp file, syncs it, then renames it into place, so the file either holds
/// the whole map or does not exist.
fn save_map(path: &Path, map: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("bin.tmp");
    let mut file = fs::File::create(&temp)?;
    file.write_all(map)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    // Sync the directory too, so the rename survives a power cut.
    if let Some(dir) = path.parent() {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}
