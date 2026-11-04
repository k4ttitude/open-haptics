//! A fake Apex 6 Pro built from the vault note "Apex 6 haptics protocol" alone. It answers the
//! read commands, keeps the live motor mode of each target, logs every written frame and takes
//! injected faults. It keeps its own copy of the RAM 6 layout so it checks `Session` instead of
//! sharing its code.
use super::reply;
use open_haptics::hidraw::Transport;
use open_haptics::protocol::{
    CMD_CONTROL, CMD_INFO, CMD_MODE, CMD_MOTOR_MAP, CMD_MOTOR_STREAM, CMD_SLOT, CMD_UUID,
    FRAME_LEN, NEUTRAL,
};
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// My pad's UUID (captured `0x04` reply, `tests/common/mod.rs`).
pub const UUID_HEX: &str = "1420c46eaa1e00000000a8d704000000";
pub const UUID: [u8; 16] = [
    0x14, 0x20, 0xc4, 0x6e, 0xaa, 0x1e, 0x00, 0x00, 0x00, 0x00, 0xa8, 0xd7, 0x04, 0x00, 0x00, 0x00,
];
pub const OTHER_UUID: [u8; 16] = [0xEE; 16];

/// My pad's RAM 6 (captured `0xA3` chunks): layout 2, both triggers mode 2 with `00 40 06`, both
/// grips mode 1 with `40`.
pub const MAP: [u8; 64] = {
    let mut m = [0u8; 64];
    m[0] = 0x02;
    m[1] = 0x02;
    m[2] = 0x00;
    m[3] = 0x40;
    m[4] = 0x06;
    m[12] = 0x02;
    m[13] = 0x00;
    m[14] = 0x40;
    m[15] = 0x06;
    m[23] = 0x01;
    m[24] = 0x40;
    m[25] = 0x01;
    m[26] = 0x40;
    m
};

/// Feature byte with `featureHaptic` (bit 7) set (note, item 2).
pub const LINEAR_GRIPS: u8 = 0x80 | 0x01;
/// Same pad without `featureHaptic`.
pub const ROTARY_GRIPS: u8 = 0x01;

/// RAM 6 layout 2: target, mode offset, param bytes kept for each mode (note, item 4).
const ENTRIES: [(u8, usize, &[usize]); 4] = [
    (0x00, 1, &[0, 10, 3]),
    (0x01, 12, &[0, 10, 3]),
    (0x10, 23, &[0, 1]),
    (0x11, 25, &[0, 1]),
];
/// Mode 2 on a grip is the streaming mode `01 12 02 40 00` sets (note, item 5).
const GRIP_STREAMING: u8 = 0x02;

pub struct PadState {
    /// Info reply payload byte 24 (note, item 2).
    pub features: u8,
    pub uuid: [u8; 16],
    pub slot: u8,
    pub ram6: [u8; 64],
    /// Live mode byte plus params for targets 00, 01, 10 and 11.
    pub modes: BTreeMap<u8, Vec<u8>>,
    /// Every frame written, in order, including the ones that a fault refused.
    pub log: Vec<[u8; FRAME_LEN]>,
    /// `(n, status)`: the nth `0x53` write (from 1) gets this ACK status and changes nothing.
    pub nack_mode_write: Option<(usize, u8)>,
    /// The pad unplugs once this many writes went through: later writes and reads fail.
    pub disconnect_after_writes: Option<usize>,
    /// When set, the pad reads this file as the first `0x53` arrives.
    pub watch_file: Option<PathBuf>,
    /// What `watch_file` held at the first `0x53`: `Some(None)` if it did not exist.
    pub file_at_first_mode_write: Option<Option<Vec<u8>>>,
    replies: VecDeque<Vec<u8>>,
    mode_writes: usize,
}

/// A cloneable handle; the test keeps one while `Session` owns another.
#[derive(Clone)]
pub struct FakePad(Arc<Mutex<PadState>>);

impl FakePad {
    pub fn new() -> Self {
        Self::with_map(MAP)
    }

    /// A pad whose RAM 6 holds `ram6`; live modes start as RAM 6 describes them.
    pub fn with_map(ram6: [u8; 64]) -> Self {
        Self(Arc::new(Mutex::new(PadState {
            features: LINEAR_GRIPS,
            uuid: UUID,
            slot: 0,
            ram6,
            modes: modes_from_map(&ram6),
            log: Vec::new(),
            nack_mode_write: None,
            disconnect_after_writes: None,
            watch_file: None,
            file_at_first_mode_write: None,
            replies: VecDeque::new(),
            mode_writes: 0,
        })))
    }

    pub fn state(&self) -> MutexGuard<'_, PadState> {
        self.0.lock().expect("FakePad lock")
    }

    pub fn modes(&self) -> BTreeMap<u8, Vec<u8>> {
        self.state().modes.clone()
    }

    /// Each written `0x53` and `0xFC` frame as "cmd: payload" in hex, for example `53: 01 02 00`.
    pub fn writes(&self) -> Vec<String> {
        let mut out = Vec::new();
        for frame in &self.state().log {
            if frame[2] == CMD_MODE || frame[2] == CMD_CONTROL {
                out.push(format!("{:02x}: {}", frame[2], hex(payload(frame))));
            }
        }
        out
    }

    pub fn count(&self, cmd: u8) -> usize {
        self.state().log.iter().filter(|f| f[2] == cmd).count()
    }
}

impl Default for FakePad {
    fn default() -> Self {
        Self::new()
    }
}

pub fn hex(bytes: &[u8]) -> String {
    let parts: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    parts.join(" ")
}

/// Request payload: frame bytes 4 on, length byte 3 minus 2 (note, "Frame format"). The `0xFC`
/// frame's byte 3 is 3, so this gives its code byte.
fn payload(frame: &[u8; FRAME_LEN]) -> &[u8] {
    let len = usize::from(frame[3].saturating_sub(2)).min(FRAME_LEN - 5);
    &frame[4..4 + len]
}

fn modes_from_map(ram6: &[u8; 64]) -> BTreeMap<u8, Vec<u8>> {
    let mut modes = BTreeMap::new();
    for (target, offset, kept) in ENTRIES {
        let mode = ram6[offset];
        let params = kept.get(usize::from(mode)).copied().unwrap_or(0);
        modes.insert(target, ram6[offset..offset + 1 + params].to_vec());
    }
    modes
}

/// Target 02 means both triggers and 12 both grips (note, item 5; 02 is inferred there).
fn targets(target: u8) -> Vec<u8> {
    match target {
        0x02 => vec![0x00, 0x01],
        0x12 => vec![0x10, 0x11],
        t => vec![t],
    }
}

fn disconnected() -> io::Error {
    io::Error::from_raw_os_error(19) // ENODEV, what hidraw returns after an unplug
}

impl PadState {
    fn is_disconnected(&self) -> bool {
        self.disconnect_after_writes
            .is_some_and(|n| self.log.len() >= n)
    }

    fn handle(&mut self, frame: &[u8; FRAME_LEN]) -> io::Result<()> {
        match frame[2] {
            CMD_INFO => {
                let mut p = [0u8; 25];
                p[24] = self.features;
                self.replies.push_back(reply(CMD_INFO, &p));
            }
            CMD_UUID => self.replies.push_back(reply(CMD_UUID, &self.uuid)),
            CMD_SLOT => self.replies.push_back(reply(CMD_SLOT, &[self.slot])),
            CMD_MOTOR_MAP => self.read_chunk(frame),
            CMD_MODE => self.mode_write(frame),
            CMD_MOTOR_STREAM => return self.motor_stream(frame),
            CMD_CONTROL => {}
            other => {
                return Err(io::Error::other(format!(
                    "FakePad: unknown command {other:#04x}"
                )));
            }
        }
        Ok(())
    }

    /// Request `00 06 idx 14`; reply RAM id, slot, total u16 LE, idx, count, data (note, item 4).
    fn read_chunk(&mut self, frame: &[u8; FRAME_LEN]) {
        let index = frame[6];
        let start = usize::from(index) * 20;
        let end = (start + 20).min(64);
        let mut p = vec![6, self.slot, 64, 0, index, (end - start) as u8];
        p.extend_from_slice(&self.ram6[start..end]);
        self.replies.push_back(reply(CMD_MOTOR_MAP, &p));
    }

    /// `01 target mode params`; ACK status at reply frame byte 5 (note, item 1).
    fn mode_write(&mut self, frame: &[u8; FRAME_LEN]) {
        self.mode_writes += 1;
        if self.mode_writes == 1
            && let Some(path) = &self.watch_file
        {
            self.file_at_first_mode_write = Some(std::fs::read(path).ok());
        }
        let status = match self.nack_mode_write {
            Some((n, status)) if n == self.mode_writes => status,
            _ => 0,
        };
        let p = payload(frame).to_vec();
        if status == 0 && p.len() >= 3 && p[0] == 0x01 {
            for target in targets(p[1]) {
                self.modes.insert(target, p[2..].to_vec());
            }
        }
        self.replies.push_back(reply(CMD_MODE, &[status]));
    }

    /// Grip samples are bytes 1 and 2 of each triple (note, item 7).
    fn motor_stream(&self, frame: &[u8; FRAME_LEN]) -> io::Result<()> {
        if frame[4] & 0x04 != 0 {
            return Err(io::Error::other("FakePad: 0x57 sets the trigger bit"));
        }
        let streaming = [0x10, 0x11]
            .iter()
            .all(|t| self.modes[t].first() == Some(&GRIP_STREAMING));
        let mut grip_samples = Vec::new();
        for triple in frame[5..29].chunks(3) {
            grip_samples.extend_from_slice(&triple[1..]);
        }
        if !streaming && grip_samples.iter().any(|b| *b != NEUTRAL) {
            return Err(io::Error::other(
                "FakePad: non-neutral 0x57 grip sample before the grip enable",
            ));
        }
        Ok(())
    }
}

impl Transport for FakePad {
    fn write(&mut self, frame: &[u8; FRAME_LEN]) -> io::Result<()> {
        let mut s = self.state();
        if s.is_disconnected() {
            return Err(disconnected());
        }
        s.log.push(*frame);
        s.handle(frame)
    }

    fn read(&mut self, _timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let mut s = self.state();
        if s.is_disconnected() {
            return Err(disconnected());
        }
        Ok(s.replies.pop_front())
    }
}
