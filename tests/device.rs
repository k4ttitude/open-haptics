mod common;

use common::{ScriptedTransport, reply};
use open_haptics::device::{self, MotorMapError};
use open_haptics::protocol::{CMD_INFO, CMD_MOTOR_MAP, CMD_SLOT, CMD_UUID};

const MAP: [u8; 64] = {
    let mut m = [0u8; 64];
    m[0] = 2;
    m[23] = 1;
    m[24] = 0x40;
    m[63] = 0xEE;
    m
};

/// A 0xA3 chunk reply payload: RAM 6, slot, total 64, index, count, data (note, item 4).
fn chunk(slot: u8, idx: u8) -> Vec<u8> {
    let data = &MAP[idx as usize * 20..(idx as usize * 20 + 20).min(64)];
    let mut p = vec![6, slot, 64, 0, idx, data.len() as u8];
    p.extend_from_slice(data);
    reply(CMD_MOTOR_MAP, &p)
}

fn good_read(slot: u8) -> Vec<Vec<u8>> {
    let mut reports = vec![reply(CMD_SLOT, &[slot])];
    reports.extend((0..4).map(|i| chunk(slot, i)));
    reports
}

#[test]
fn reads_the_motor_map_in_four_chunks() {
    let mut t = ScriptedTransport::new(good_read(1));
    let read = device::read_motor_map(&mut t).unwrap();
    assert_eq!(read.slot, 1);
    assert_eq!(read.map, MAP);
    let sent: Vec<(u8, Vec<u8>)> = t.written.iter().map(|f| (f[2], f[4..8].to_vec())).collect();
    assert_eq!(sent[0].0, CMD_SLOT);
    assert_eq!(sent[1], (CMD_MOTOR_MAP, vec![0, 6, 0, 20]));
    assert_eq!(sent[4], (CMD_MOTOR_MAP, vec![0, 6, 3, 20]));
}

#[test]
fn retries_the_whole_read_after_a_bad_chunk() {
    let mut reports = vec![reply(CMD_SLOT, &[1]), chunk(2, 0)]; // slot differs from 0xA1
    reports.extend(good_read(1));
    let mut t = ScriptedTransport::new(reports);
    assert_eq!(device::read_motor_map(&mut t).unwrap().map, MAP);
}

#[test]
fn gives_up_after_three_attempts() {
    let mut reports = Vec::new();
    for _ in 0..3 {
        reports.push(reply(CMD_SLOT, &[1]));
        reports.push(chunk(2, 0));
    }
    reports.extend(good_read(1));
    let mut t = ScriptedTransport::new(reports);
    assert!(matches!(
        device::read_motor_map(&mut t),
        Err(MotorMapError::SlotMismatch { .. })
    ));
}

#[test]
fn rejects_a_wrong_total_ram_id_or_index() {
    for bad in [
        reply(CMD_MOTOR_MAP, &[6, 1, 65, 0, 0, 20]),
        reply(CMD_MOTOR_MAP, &[5, 1, 64, 0, 0, 20]),
        reply(CMD_MOTOR_MAP, &[6, 1, 64, 0, 1, 20]),
        reply(CMD_MOTOR_MAP, &[6, 1, 64, 0, 0, 21]),
    ] {
        let mut reports = Vec::new();
        for _ in 0..3 {
            reports.push(reply(CMD_SLOT, &[1]));
            reports.push(bad.clone());
        }
        let mut t = ScriptedTransport::new(reports);
        assert!(device::read_motor_map(&mut t).is_err());
    }
}

#[test]
fn rejects_a_slot_above_three() {
    let reports = vec![reply(CMD_SLOT, &[4]); 3];
    let mut t = ScriptedTransport::new(reports);
    assert!(matches!(
        device::read_motor_map(&mut t),
        Err(MotorMapError::BadSlot(4))
    ));
}

#[test]
fn parses_device_info_and_feature_bits() {
    let mut p = [0u8; 26];
    p[0] = 7; // deviceId
    p[1] = 2; // connectionMode
    p[10..12].copy_from_slice(&0x0102u16.to_be_bytes());
    p[24] = 0b1001_0001;
    let mut t = ScriptedTransport::new(vec![reply(CMD_INFO, &p)]);
    let info = device::read_info(&mut t).unwrap();
    assert_eq!((info.device_id, info.connection_mode), (7, 2));
    assert_eq!(info.fw_main_version, 0x0102);
    assert!(info.feature_haptic());
    assert_eq!(info.feature_names(), ["imu", "triggerHaptic", "haptic"]);
}

#[test]
fn reads_the_uuid_as_32_hex_digits() {
    let p: Vec<u8> = (0..16).map(|i| 0xA0 + i).collect();
    let mut t = ScriptedTransport::new(vec![reply(CMD_UUID, &p)]);
    assert_eq!(
        device::read_uuid(&mut t).unwrap(),
        "a0a1a2a3a4a5a6a7a8a9aaabacadaeaf"
    );
}

#[test]
fn reads_the_captured_replies_from_my_pad() {
    use common::captured::{self, INFO, MOTOR_MAP_CHUNKS, SLOT, UUID};
    let mut t = ScriptedTransport::new(vec![captured::bytes(INFO)]);
    let info = device::read_info(&mut t).unwrap();
    assert!(info.feature_haptic());
    assert_eq!(info.device_id, 0x98);

    let mut t = ScriptedTransport::new(vec![captured::bytes(UUID)]);
    assert_eq!(
        device::read_uuid(&mut t).unwrap(),
        "1420c46eaa1e00000000a8d704000000"
    );

    let mut reports = vec![captured::bytes(SLOT)];
    reports.extend(MOTOR_MAP_CHUNKS.map(captured::bytes));
    let mut t = ScriptedTransport::new(reports);
    let read = device::read_motor_map(&mut t).unwrap();
    assert_eq!(read.slot, 0);
    assert_eq!(&read.map[..5], &[0x02, 0x02, 0x00, 0x40, 0x06]);
    // Left and right grip entries: mode 1, param 0x40 (note, item 4, offsets 23 to 26).
    assert_eq!(&read.map[23..27], &[0x01, 0x40, 0x01, 0x40]);
}
