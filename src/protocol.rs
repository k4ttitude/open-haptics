//! Apex 6 HID frame codec. Offsets and constants cite the vault note "Apex 6 haptics protocol".
use std::fmt;

const MAGIC: [u8; 2] = [0x5A, 0xA5];
/// Frame length for payloads up to `MAX_PAYLOAD` (note, "Frame format", builder 0x14028ebf0).
pub const FRAME_LEN: usize = 32;
pub const MAX_PAYLOAD: usize = FRAME_LEN - 5;
/// Payload starts here in a request, one byte earlier than in a reply (note, "Frame format").
const REQUEST_PAYLOAD_START: usize = 4;
/// The engine hands callers reply bytes from here (note, item 1, 0x14028294d).
const REPLY_PAYLOAD_START: usize = 5;

pub const CMD_INFO: u8 = 0x01;
pub const CMD_UUID: u8 = 0x04;
pub const CMD_SLOT: u8 = 0xA1;
pub const CMD_MODE: u8 = 0x53;
pub const CMD_MOTOR_MAP: u8 = 0xA3;
pub const CMD_MOTOR_STREAM: u8 = 0x57;
pub const CMD_CONTROL: u8 = 0xFC;

/// Area 1 holds the realtime motor modes; payloads are `01 target mode params` (note, item 6
/// step 7, 0x140284109).
pub const AREA_REALTIME: u8 = 0x01;
/// Target 02 (inferred: both triggers) to mode 0, the grip-only start write and stop step 3
/// (note, item 5, 0x14027e54e; item 6, 0x14027e830).
pub const TRIGGERS_OFF: [u8; 3] = [AREA_REALTIME, 0x02, 0x00];
/// Both grips (target 12) to mode 2 with params `40 00` (note, item 5, 0x14027e5f2).
pub const GRIPS_STREAM: [u8; 5] = [AREA_REALTIME, 0x12, 0x02, 0x40, 0x00];
/// Both grips to mode 0, stop step 4 (note, item 6, 0x14027e943).
pub const GRIPS_OFF: [u8; 3] = [AREA_REALTIME, 0x12, 0x00];
/// `0x53` ACK status for success; the status is reply payload byte 0 (note, item 1, 0x1402855e2).
pub const ACK_OK: u8 = 0x00;
/// `0xFC` codes sent at start, in order (note, item 5, 0x1402861eb, 0x1402861fa).
pub const CONTROL_START: [u8; 2] = [0x01, 0x03];
/// `0xFC` code sent at stop (note, item 6 step 2, 0x140104818).
pub const CONTROL_STOP: u8 = 0x02;

/// Byte that quantizes to silence (note, item 8, 0.0 gives 0x80).
pub const NEUTRAL: u8 = 0x80;
/// Samples per `0x57` packet (note, item 7).
pub const SAMPLES_PER_PACKET: usize = 8;
/// Bit 7 always set, bits 3 and 4 enable the left and right grips, selector bits clear in
/// grip-only use (note, item 7, 0x140285c1f to 0x140285c3c).
const GRIP_CONTROL: u8 = 0x98;

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    PayloadTooLong,
    BadMagic,
    BadChecksum,
    WrongCommand { expected: u8, got: u8 },
    Short,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLong => write!(f, "payload is longer than {MAX_PAYLOAD} bytes"),
            Self::BadMagic => write!(f, "report does not start with 5A A5"),
            Self::BadChecksum => write!(f, "report checksum does not match its contents"),
            Self::WrongCommand { expected, got } => {
                write!(
                    f,
                    "expected a reply to command {expected:#04x}, got {got:#04x}"
                )
            }
            Self::Short => write!(f, "report is too short to hold a reply"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Sum of bytes 2 to the one before the checksum, mod 256 (note, "Frame format").
fn checksum(body: &[u8]) -> u8 {
    body.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
}

pub fn frame(cmd: u8, payload: &[u8]) -> Result<[u8; FRAME_LEN], ProtocolError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(ProtocolError::PayloadTooLong);
    }
    Ok(assemble(cmd, payload))
}

/// Lays out a frame; callers guarantee `payload.len() <= MAX_PAYLOAD`.
fn assemble(cmd: u8, payload: &[u8]) -> [u8; FRAME_LEN] {
    let mut out = [0; FRAME_LEN];
    out[..2].copy_from_slice(&MAGIC);
    out[2] = cmd;
    // Length byte counts the payload plus two (note, "Frame format"); at most 29, so no overflow.
    out[3] = payload.len() as u8 + 2;
    out[REQUEST_PAYLOAD_START..REQUEST_PAYLOAD_START + payload.len()].copy_from_slice(payload);
    out[FRAME_LEN - 1] = checksum(&out[2..FRAME_LEN - 1]);
    out
}

/// Builds the short `0xFC` frame `5A A5 FC 03 code chk`: the checksum is `code - 1` at byte 5
/// and bytes 6 to 31 are zero (note, "Frame format", 0x140284de0).
pub fn control_frame(code: u8) -> [u8; FRAME_LEN] {
    let mut out = [0; FRAME_LEN];
    out[..2].copy_from_slice(&MAGIC);
    out[2] = CMD_CONTROL;
    out[3] = 0x03;
    out[4] = code;
    out[5] = code.wrapping_sub(1);
    out
}

/// Maps [-1, 1] to 0..=255 as the engine does: clamp, add 1, scale by 127.5 in f32, round half
/// away from zero (note, item 8, 0x14028f160). NaN becomes neutral instead of the engine's 0x00.
pub fn quantize(x: f32) -> u8 {
    if x.is_nan() {
        return NEUTRAL;
    }
    ((x.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8
}

/// Builds a grip-only `0x57` packet from the first eight samples of each side; missing samples
/// are neutral.
pub fn motor_packet(left: &[f32], right: &[f32]) -> [u8; FRAME_LEN] {
    let sample = |side: &[f32], i: usize| side.get(i).map_or(NEUTRAL, |x| quantize(*x));
    const PAYLOAD_LEN: usize = 1 + 3 * SAMPLES_PER_PACKET;
    const _: () = assert!(PAYLOAD_LEN <= MAX_PAYLOAD);
    let mut payload = [NEUTRAL; PAYLOAD_LEN];
    payload[0] = GRIP_CONTROL;
    for i in 0..SAMPLES_PER_PACKET {
        // Triples are (trigger, left grip, right grip); the trigger column stays neutral.
        payload[1 + 3 * i + 1] = sample(left, i);
        payload[1 + 3 * i + 2] = sample(right, i);
    }
    // Bytes 29 and 30 stay zero because `frame` zero-pads (note, item 7).
    assemble(CMD_MOTOR_STREAM, &payload)
}

/// The generic scan wants byte 3 non-zero (0x140282946). The 0xA3 parser wants it zero
/// (0x14028eff7, note item 4), and my pad's replies agree.
fn length_byte_ok(cmd: u8, byte3: u8) -> bool {
    (byte3 == 0) == (cmd == CMD_MOTOR_MAP)
}

/// Finds the reply to `cmd` and returns its payload, frame bytes 5 to 30 (note, item 1).
pub fn parse_reply(cmd: u8, report: &[u8]) -> Result<&[u8], ProtocolError> {
    let mut other_command = None;
    // The engine scans every offset for `5A A5 cmd` with byte 3 non-zero (0x140282900).
    for (start, window) in report.windows(4).enumerate() {
        if window[..2] != MAGIC || !length_byte_ok(cmd, window[3]) {
            continue;
        }
        if window[2] != cmd {
            other_command.get_or_insert(window[2]);
            continue;
        }
        let frame = report
            .get(start..start + FRAME_LEN)
            .ok_or(ProtocolError::Short)?;
        // Only the 0xA3 parser verifies the checksum (0x14028efc0) and requires byte 4 == 0
        // (0x14028effd).
        if cmd == CMD_MOTOR_MAP && frame[4] != 0 {
            return Err(ProtocolError::BadMagic);
        }
        if cmd == CMD_MOTOR_MAP && checksum(&frame[2..FRAME_LEN - 1]) != frame[FRAME_LEN - 1] {
            return Err(ProtocolError::BadChecksum);
        }
        return Ok(&frame[REPLY_PAYLOAD_START..FRAME_LEN - 1]);
    }
    Err(match other_command {
        Some(got) => ProtocolError::WrongCommand { expected: cmd, got },
        None => ProtocolError::BadMagic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn info_request_matches_engine_layout() {
        assert_eq!(
            hex(&frame(0x01, &[]).unwrap()),
            "5aa5010200000000000000000000000000000000000000000000000000000003"
        );
    }

    #[test]
    fn neutral_grip_packet() {
        assert_eq!(
            hex(&motor_packet(&[], &[])),
            "5aa5571b9880808080808080808080808080808080808080808080808000000a"
        );
    }

    #[test]
    fn full_scale_grip_packet() {
        assert_eq!(
            hex(&motor_packet(&[1.0; 8], &[-1.0; 8])),
            "5aa5571b9880ff0080ff0080ff0080ff0080ff0080ff0080ff0080ff00000002"
        );
    }

    #[test]
    fn quantize_matches_engine() {
        for (x, b) in [
            (0.0, 128),
            (1.0, 255),
            (-1.0, 0),
            (2.0, 255),
            (-2.0, 0),
            (0.5, 191),
            (-0.5, 64),
        ] {
            assert_eq!(quantize(x), b);
        }
        // Deliberate deviation: the engine yields 0x00 for NaN (note section 8), we send neutral.
        assert_eq!(quantize(f32::NAN), 128);
    }

    /// Worked `0xFC` frames from the note, items 5 and 6.
    #[test]
    fn control_frames_match_engine_layout() {
        let zeros = "00".repeat(26);
        assert_eq!(hex(&control_frame(1)), format!("5aa5fc030100{zeros}"));
        assert_eq!(hex(&control_frame(2)), format!("5aa5fc030201{zeros}"));
        assert_eq!(hex(&control_frame(3)), format!("5aa5fc030302{zeros}"));
    }

    /// Worked `0x53` frames from the note, items 5 and 6: (payload, length byte, checksum).
    #[test]
    fn mode_frames_match_engine_layout() {
        for (payload, len, chk) in [
            (&TRIGGERS_OFF[..], 0x05, 0x5B),
            (&GRIPS_STREAM[..], 0x07, 0xAF),
            (&GRIPS_OFF[..], 0x05, 0x6B),
        ] {
            let f = frame(CMD_MODE, payload).unwrap();
            assert_eq!(f[..4], [0x5A, 0xA5, CMD_MODE, len]);
            assert_eq!(&f[4..4 + payload.len()], payload);
            assert!(f[4 + payload.len()..31].iter().all(|b| *b == 0));
            assert_eq!(f[31], chk);
        }
    }

    #[test]
    fn rejects_payload_over_27_bytes() {
        assert!(matches!(
            frame(0x57, &[0; 28]),
            Err(ProtocolError::PayloadTooLong)
        ));
    }
}
