mod common;

use common::captured::{self, INFO, MOTOR_MAP_CHUNKS};
use open_haptics::protocol::{CMD_INFO, CMD_MOTOR_MAP, CMD_UUID, ProtocolError, parse_reply};

#[test]
fn returns_frame_bytes_5_to_30_of_a_captured_reply() {
    let report = captured::bytes(INFO);
    let payload = parse_reply(CMD_INFO, &report).unwrap();
    assert_eq!(payload.len(), 26);
    assert_eq!(&payload[..2], &[0x98, 0x01]);
    assert_eq!(payload[24], 0x9F);
}

#[test]
fn accepts_a_long_report_with_zero_padding() {
    // The engine reads at least 64 bytes (note, item 1, 0x1402825f7).
    let mut report = captured::bytes(INFO);
    report.resize(64, 0);
    assert_eq!(parse_reply(CMD_INFO, &report).unwrap().len(), 26);
}

#[test]
fn finds_a_frame_after_a_report_id_byte() {
    // The engine scans every offset (note, item 1, 0x140282900).
    let mut report = vec![0];
    report.extend(captured::bytes(INFO));
    assert_eq!(parse_reply(CMD_INFO, &report).unwrap()[0], 0x98);
}

#[test]
fn accepts_a3_chunks_whose_byte_3_is_zero() {
    // The 0xA3 parser requires byte 3 == 0 where the generic scan requires it non-zero
    // (note, item 4, 0x14028eff7).
    for chunk in MOTOR_MAP_CHUNKS {
        let report = captured::bytes(chunk);
        assert_eq!(parse_reply(CMD_MOTOR_MAP, &report).unwrap().len(), 26);
    }
}

#[test]
fn checks_the_checksum_only_for_a3() {
    // Only the 0xA3 parser verifies it (note, item 1, 0x14028efc0).
    let mut info = captured::bytes(INFO);
    info[31] ^= 1;
    assert!(parse_reply(CMD_INFO, &info).is_ok());
    let mut a3 = captured::bytes(MOTOR_MAP_CHUNKS[0]);
    a3[31] ^= 1;
    assert_eq!(
        parse_reply(CMD_MOTOR_MAP, &a3),
        Err(ProtocolError::BadChecksum)
    );
}

#[test]
fn rejects_a_frame_too_close_to_the_end() {
    let report = captured::bytes(INFO);
    assert_eq!(
        parse_reply(CMD_INFO, &report[..31]),
        Err(ProtocolError::Short)
    );
    let mut late = vec![0; 10];
    late.extend_from_slice(&report[..30]);
    assert_eq!(parse_reply(CMD_INFO, &late), Err(ProtocolError::Short));
}

#[test]
fn reports_another_commands_frame_as_wrong_command() {
    assert_eq!(
        parse_reply(CMD_UUID, &captured::bytes(INFO)),
        Err(ProtocolError::WrongCommand {
            expected: CMD_UUID,
            got: CMD_INFO
        })
    );
}

#[test]
fn ignores_a_generic_frame_with_zero_length_byte() {
    let mut report = captured::bytes(INFO);
    report[3] = 0;
    assert_eq!(parse_reply(CMD_INFO, &report), Err(ProtocolError::BadMagic));
}

#[test]
fn without_magic_is_bad_magic() {
    assert_eq!(
        parse_reply(CMD_INFO, &[0; 64]),
        Err(ProtocolError::BadMagic)
    );
    assert_eq!(parse_reply(CMD_INFO, &[]), Err(ProtocolError::BadMagic));
}

#[test]
fn a3_requires_bytes_3_and_4_to_be_zero() {
    // The 0xA3 parser checks both (note, item 4, 0x14028eff7 and 0x14028effd). The checksum is
    // recomputed so that only the zero check can reject the frame.
    for byte in [3, 4] {
        let mut report = captured::bytes(MOTOR_MAP_CHUNKS[0]);
        report[byte] = 1;
        report[31] = report[2..31].iter().fold(0u8, |s, b| s.wrapping_add(*b));
        assert_eq!(
            parse_reply(CMD_MOTOR_MAP, &report),
            Err(ProtocolError::BadMagic),
            "byte {byte}"
        );
    }
}
