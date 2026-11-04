//! Read-only device queries: info `0x01`, UUID `0x04`, slot `0xA1` and the `0xA3` motor map.
use crate::hidraw::{AskError, Transport, ask};
use crate::protocol::{CMD_INFO, CMD_MOTOR_MAP, CMD_SLOT, CMD_UUID};
use std::fmt;
use std::time::Duration;

/// Reply timeout for `0x01`, `0x04`, `0xA1` and `0xA3` (note, item 1, 0x14028357e, 0x140283899).
const REPLY_TIMEOUT: Duration = Duration::from_millis(0x5dc);
/// The info parser needs 25 reply-payload bytes (note, item 2, 0x140278455).
const INFO_MIN_LEN: usize = 25;
const FEATURE_BYTE: usize = 24;
const UUID_LEN: usize = 16;
/// Feature byte bit names, bit 0 first (note, item 2, 0x14027853e to 0x1402785c4).
const FEATURES: [&str; 8] = [
    "imu",
    "led",
    "switch",
    "screen",
    "triggerHaptic",
    "triggerVibrate",
    "audio",
    "haptic",
];
const FEATURE_HAPTIC: u8 = 7;

/// The engine rejects slots above 3 (note, item 3, 0x1402837d7).
const MAX_SLOT: u8 = 3;
/// `0xA3` sub-command 0 reads a chunk; the RAM id and chunk size follow (note, item 4, 0x140283800).
const READ_CHUNK: u8 = 0;
const MOTOR_MAP_RAM: u8 = 6;
const CHUNK_LEN: usize = 20;
/// The whole read is exactly 64 bytes (note, item 4, 0x1402839a6).
pub const MOTOR_MAP_LEN: usize = 64;
/// The engine retries the whole `0xA1` plus `0xA3` read up to 3 times (0x140283e51).
const MOTOR_MAP_ATTEMPTS: usize = 3;

#[derive(Debug)]
pub enum InfoError {
    Ask(AskError),
    Short { need: usize, got: usize },
}

impl fmt::Display for InfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ask(e) => e.fmt(f),
            Self::Short { need, got } => write!(f, "reply holds {got} bytes, need {need}"),
        }
    }
}

impl std::error::Error for InfoError {}

impl From<AskError> for InfoError {
    fn from(e: AskError) -> Self {
        Self::Ask(e)
    }
}

pub struct DeviceInfo {
    pub device_id: u8,
    pub connection_mode: u8,
    /// Big-endian u16 at reply-payload offset 10 (note, item 2, 0x1402784c5).
    pub fw_main_version: u16,
    features: u8,
}

impl DeviceInfo {
    fn parse(payload: &[u8]) -> Result<Self, InfoError> {
        if payload.len() < INFO_MIN_LEN {
            return Err(InfoError::Short {
                need: INFO_MIN_LEN,
                got: payload.len(),
            });
        }
        Ok(Self {
            device_id: payload[0],
            connection_mode: payload[1],
            fw_main_version: u16::from_be_bytes([payload[10], payload[11]]),
            features: payload[FEATURE_BYTE],
        })
    }

    /// The engine treats the grips as present only when this is set (note, item 2).
    pub fn feature_haptic(&self) -> bool {
        self.has_feature(FEATURE_HAPTIC)
    }

    pub fn feature_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for (bit, name) in FEATURES.iter().enumerate() {
            if self.has_feature(bit as u8) {
                names.push(*name);
            }
        }
        names
    }

    fn has_feature(&self, bit: u8) -> bool {
        self.features >> bit & 1 == 1
    }
}

pub fn read_info<T: Transport>(t: &mut T) -> Result<DeviceInfo, InfoError> {
    DeviceInfo::parse(&ask(t, CMD_INFO, &[], REPLY_TIMEOUT)?)
}

/// Formats reply bytes 0 to 15 as lowercase hex (note, item 2, 0x140278790).
pub fn read_uuid<T: Transport>(t: &mut T) -> Result<String, InfoError> {
    let payload = ask(t, CMD_UUID, &[], REPLY_TIMEOUT)?;
    let Some(uuid) = payload.get(..UUID_LEN) else {
        return Err(InfoError::Short {
            need: UUID_LEN,
            got: payload.len(),
        });
    };
    Ok(uuid.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug)]
pub enum MotorMapError {
    Ask(AskError),
    Short { got: usize },
    BadSlot(u8),
    WrongRam(u8),
    SlotMismatch { expected: u8, got: u8 },
    WrongTotal(u16),
    WrongIndex { expected: u8, got: u8 },
    BadCount(u8),
    WrongLength(usize),
}

impl fmt::Display for MotorMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ask(e) => e.fmt(f),
            Self::Short { got } => write!(f, "motor map reply holds only {got} bytes"),
            Self::BadSlot(slot) => write!(
                f,
                "pad reports profile slot {slot}, expected 0 to {MAX_SLOT}"
            ),
            Self::WrongRam(id) => write!(
                f,
                "motor map reply is for RAM {id}, expected {MOTOR_MAP_RAM}"
            ),
            Self::SlotMismatch { expected, got } => {
                write!(
                    f,
                    "motor map chunk is for slot {got}, the active slot is {expected}"
                )
            }
            Self::WrongTotal(total) => {
                write!(
                    f,
                    "motor map reply announces {total} bytes, expected {MOTOR_MAP_LEN}"
                )
            }
            Self::WrongIndex { expected, got } => {
                write!(
                    f,
                    "motor map chunk {got} arrived, expected chunk {expected}"
                )
            }
            Self::BadCount(count) => {
                write!(
                    f,
                    "motor map chunk holds {count} bytes, at most {CHUNK_LEN} allowed"
                )
            }
            Self::WrongLength(len) => {
                write!(f, "motor map came to {len} bytes, expected {MOTOR_MAP_LEN}")
            }
        }
    }
}

impl std::error::Error for MotorMapError {}

impl From<AskError> for MotorMapError {
    fn from(e: AskError) -> Self {
        Self::Ask(e)
    }
}

pub struct MotorMap {
    pub slot: u8,
    pub map: [u8; MOTOR_MAP_LEN],
}

/// Reads the active slot (`0xA1`), then the 64-byte motor map (`0xA3`) in chunks, validating
/// each reply. Retries the whole read up to 3 times and returns the last error.
pub fn read_motor_map<T: Transport>(t: &mut T) -> Result<MotorMap, MotorMapError> {
    let mut result = read_motor_map_once(t);
    for _ in 1..MOTOR_MAP_ATTEMPTS {
        if result.is_ok() {
            break;
        }
        result = read_motor_map_once(t);
    }
    result
}

fn read_motor_map_once<T: Transport>(t: &mut T) -> Result<MotorMap, MotorMapError> {
    let slot = read_slot(t)?;
    let mut map = Vec::with_capacity(MOTOR_MAP_LEN);
    let mut index = 0u8;
    while map.len() < MOTOR_MAP_LEN {
        map.extend_from_slice(&read_chunk(t, slot, index)?);
        index += 1;
    }
    let len = map.len();
    let map = map
        .try_into()
        .map_err(|_| MotorMapError::WrongLength(len))?;
    Ok(MotorMap { slot, map })
}

fn read_slot<T: Transport>(t: &mut T) -> Result<u8, MotorMapError> {
    let payload = ask(t, CMD_SLOT, &[], REPLY_TIMEOUT)?;
    match payload.first() {
        Some(&slot) if slot <= MAX_SLOT => Ok(slot),
        Some(&slot) => Err(MotorMapError::BadSlot(slot)),
        None => Err(MotorMapError::Short { got: 0 }),
    }
}

/// Reply payload layout: RAM id, slot, total (u16 LE), chunk index, count, data (note, item 4).
fn read_chunk<T: Transport>(t: &mut T, slot: u8, index: u8) -> Result<Vec<u8>, MotorMapError> {
    let request = [READ_CHUNK, MOTOR_MAP_RAM, index, CHUNK_LEN as u8];
    let p = ask(t, CMD_MOTOR_MAP, &request, REPLY_TIMEOUT)?;
    let data_start = 6;
    if p.len() < data_start + CHUNK_LEN {
        return Err(MotorMapError::Short { got: p.len() });
    }
    if p[0] != MOTOR_MAP_RAM {
        return Err(MotorMapError::WrongRam(p[0]));
    }
    if p[1] != slot {
        return Err(MotorMapError::SlotMismatch {
            expected: slot,
            got: p[1],
        });
    }
    let total = u16::from_le_bytes([p[2], p[3]]);
    if usize::from(total) != MOTOR_MAP_LEN {
        return Err(MotorMapError::WrongTotal(total));
    }
    if p[4] != index {
        return Err(MotorMapError::WrongIndex {
            expected: index,
            got: p[4],
        });
    }
    let count = p[5];
    if usize::from(count) > CHUNK_LEN {
        return Err(MotorMapError::BadCount(count));
    }
    Ok(p[data_start..data_start + usize::from(count)].to_vec())
}
