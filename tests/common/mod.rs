use open_haptics::hidraw::Transport;
use open_haptics::protocol::FRAME_LEN;
use std::collections::VecDeque;
use std::io;
use std::time::Duration;

/// Plays back queued input reports and records writes. An empty queue reads as silence.
#[derive(Default)]
pub struct ScriptedTransport {
    pub reports: VecDeque<Vec<u8>>,
    pub written: Vec<[u8; FRAME_LEN]>,
}

impl ScriptedTransport {
    pub fn new(reports: Vec<Vec<u8>>) -> Self {
        Self {
            reports: reports.into(),
            written: Vec::new(),
        }
    }
}

impl Transport for ScriptedTransport {
    fn write(&mut self, frame: &[u8; FRAME_LEN]) -> io::Result<()> {
        self.written.push(*frame);
        Ok(())
    }

    fn read(&mut self, _timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        Ok(self.reports.pop_front())
    }
}

/// A 64-byte input report holding one frame with the given command and payload (frame bytes 5 on),
/// checksummed like a real one.
pub fn reply(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut report = vec![0u8; 64];
    report[..4].copy_from_slice(&[0x5A, 0xA5, cmd, payload.len() as u8 + 3]);
    report[5..5 + payload.len()].copy_from_slice(payload);
    report[31] = report[2..31].iter().fold(0u8, |s, b| s.wrapping_add(*b));
    report
}
