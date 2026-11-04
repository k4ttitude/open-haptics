// Each test crate uses a different part of this module.
#![allow(dead_code)]

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
    // 0xA3 replies carry 00 in byte 3 where the others carry a non-zero value (captured below).
    let byte3 = if cmd == 0xA3 {
        0
    } else {
        payload.len() as u8 + 3
    };
    report[..4].copy_from_slice(&[0x5A, 0xA5, cmd, byte3]);
    report[5..5 + payload.len()].copy_from_slice(payload);
    report[31] = report[2..31].iter().fold(0u8, |s, b| s.wrapping_add(*b));
    report
}

/// Input reports captured from my Apex 6 Pro on 2026-11-04 (usage page 0xFFA0 node). The pad
/// sends bare 32-byte frames, with no report ID and no padding.
pub mod captured {
    pub const INFO: &str = "5aa50101009801000000000845010090410491010900000144000010509f009d";
    pub const UUID: &str = "5aa50401001420c46eaa1e00000000a8d70400000000000000000000000000b6";
    pub const SLOT: &str = "5aa5a10100004c48124112411241312c0000000000000000000000000000008c";
    /// `0xA3` chunk replies for indexes 0 to 3. Note that byte 3 is `00` here.
    pub const MOTOR_MAP_CHUNKS: [&str; 4] = [
        "5aa5a3000006004000001402020040060000000000000002004006000000008f",
        "5aa5a30000060040000114000000014001400000000000020000000000000082",
        "5aa5a300000600400002140000000000000000000000000000000000000000ff",
        "5aa5a300000600400003040000000000000000000000000000000000000000f0",
    ];

    pub fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("fixture is hex"))
            .collect()
    }
}
