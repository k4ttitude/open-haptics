mod common;

use common::fake_pad::FakePad;
use common::fake_pad::{MAP, OTHER_UUID, ROTARY_GRIPS};
use open_haptics::protocol::{CMD_CONTROL, CMD_MODE, CMD_MOTOR_STREAM};
use open_haptics::session::{Session, SessionError, map_path, restore_from_file};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Writes `Session::start` sends after its reads, for my pad (note, item 5, grip-only path).
const START_WRITES: [&str; 4] = ["53: 01 02 00", "53: 01 12 02 40 00", "fc: 01", "fc: 03"];
/// Writes `Session::stop` sends: item 6 steps 2 to 4, then step 7 from the saved map.
const STOP_WRITES: [&str; 7] = [
    "fc: 02",
    "53: 01 02 00",
    "53: 01 12 00",
    "53: 01 00 02 00 40 06",
    "53: 01 01 02 00 40 06",
    "53: 01 10 01 40",
    "53: 01 11 01 40",
];
/// Writes before the first `0x53`: `0x01`, `0x04`, `0xA1` and four `0xA3` chunks.
const READS: usize = 7;

/// `first` followed by the stop writes.
fn then_stop(first: &[&'static str]) -> Vec<&'static str> {
    [first, &STOP_WRITES[..]].concat()
}

fn setup() -> (FakePad, TempDir, PathBuf) {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let path = map_path(dir.path(), common::fake_pad::UUID_HEX);
    (pad, dir, path)
}

fn no_device_writes(pad: &FakePad) -> bool {
    pad.count(CMD_MODE) == 0 && pad.count(CMD_CONTROL) == 0 && pad.count(CMD_MOTOR_STREAM) == 0
}

#[test]
fn saves_map_before_first_mode_write() {
    let (pad, dir, path) = setup();
    pad.state().watch_file = Some(path.clone());
    let mut session = Session::start(pad.clone(), dir.path()).unwrap();
    assert_eq!(
        pad.state().file_at_first_mode_write,
        Some(Some(MAP.to_vec()))
    );
    session.stop().unwrap();
}

#[test]
fn start_then_stop_restores_original_modes() {
    let (pad, dir, path) = setup();
    let before = pad.modes();
    let mut session = Session::start(pad.clone(), dir.path()).unwrap();
    assert_eq!(pad.writes(), START_WRITES);
    assert_eq!(pad.modes()[&0x10], [0x02, 0x40, 0x00]);
    assert_eq!(pad.modes()[&0x00], [0x00]);

    session.send(&[0.5; 8], &[-0.5; 8]).unwrap();
    let last = *pad.state().log.last().unwrap();
    assert_eq!(
        last,
        open_haptics::protocol::motor_packet(&[0.5; 8], &[-0.5; 8])
    );

    session.stop().unwrap();
    assert_eq!(pad.writes(), then_stop(&START_WRITES));
    assert_eq!(pad.modes(), before);
    assert!(!path.exists(), "the map file is deleted after a clean stop");

    // A second stop, and the drop after it, write nothing more.
    let writes = pad.state().log.len();
    session.stop().unwrap();
    drop(session);
    assert_eq!(pad.state().log.len(), writes);
}

#[test]
fn drop_without_stop_restores() {
    let (pad, dir, path) = setup();
    let before = pad.modes();
    let session = Session::start(pad.clone(), dir.path()).unwrap();
    drop(session);
    assert_eq!(pad.writes(), then_stop(&START_WRITES));
    assert_eq!(pad.modes(), before);
    assert!(!path.exists());
}

#[test]
fn unknown_map_layout_refuses_without_writing() {
    let mut layout_1 = MAP;
    layout_1[0] = 0x01;
    let mut grip_mode_2 = MAP;
    grip_mode_2[25] = 0x02;
    let mut trigger_mode_3 = MAP;
    trigger_mode_3[12] = 0x03;
    for map in [layout_1, grip_mode_2, trigger_mode_3] {
        let pad = FakePad::with_map(map);
        let dir = tempfile::tempdir().unwrap();
        let result = Session::start(pad.clone(), dir.path());
        assert!(matches!(result, Err(SessionError::UnknownMapLayout)));
        assert!(no_device_writes(&pad), "{:?}", pad.writes());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn nack_during_setup_restores_and_returns_refused() {
    let (pad, dir, path) = setup();
    let before = pad.modes();
    // The second 0x53 is the grip enable, the one write whose ACK the session checks.
    pad.state().nack_mode_write = Some((2, 1));
    let result = Session::start(pad.clone(), dir.path());
    assert!(matches!(result, Err(SessionError::Refused { status: 1 })));
    assert_eq!(pad.writes(), then_stop(&START_WRITES[..2]));
    assert_eq!(pad.modes(), before);
    assert!(!path.exists());
}

#[test]
fn disconnect_mid_stream_returns_error_and_keeps_the_map_file() {
    let (pad, dir, path) = setup();
    pad.state().disconnect_after_writes = Some(READS + START_WRITES.len() + 1);
    let mut session = Session::start(pad.clone(), dir.path()).unwrap();
    session.send(&[0.1; 8], &[0.1; 8]).unwrap();
    assert!(matches!(
        session.send(&[0.1; 8], &[0.1; 8]),
        Err(SessionError::Io(_))
    ));
    assert!(session.stop().is_err());
    drop(session);
    assert_eq!(fs::read(&path).unwrap(), MAP);
}

#[test]
fn disconnect_during_setup_keeps_the_map_file() {
    let (pad, dir, path) = setup();
    pad.state().disconnect_after_writes = Some(READS + 1);
    let result = Session::start(pad.clone(), dir.path());
    assert!(matches!(
        result,
        Err(SessionError::Io(_) | SessionError::Ask(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), MAP);
}

#[test]
fn start_refuses_when_a_saved_map_exists() {
    let (pad, dir, path) = setup();
    fs::write(&path, b"map from a run that died").unwrap();
    let err = match Session::start(pad.clone(), dir.path()) {
        Err(e) => e,
        Ok(_) => panic!("start must refuse"),
    };
    assert!(matches!(&err, SessionError::SavedMapExists(p) if *p == path));
    assert!(
        err.to_string()
            .contains("run open-haptics restore --experimental first")
    );
    assert_eq!(fs::read(&path).unwrap(), b"map from a run that died");
    assert!(no_device_writes(&pad));
}

#[test]
fn restore_from_file_says_when_no_map_is_saved_for_this_pad() {
    let (mut pad, dir, path) = setup();
    fs::write(&path, MAP).unwrap();
    pad.state().uuid = OTHER_UUID;
    let result = restore_from_file(&mut pad, dir.path());
    assert!(matches!(result, Err(SessionError::NoSavedMap(_))));
    assert!(no_device_writes(&pad));
    assert_eq!(
        fs::read(&path).unwrap(),
        MAP,
        "another pad's file is untouched"
    );
}

#[test]
fn restore_from_file_restores_and_deletes_the_file() {
    let (mut pad, dir, path) = setup();
    let before = pad.modes();
    // An earlier run that died without stopping: modes changed, map file left behind.
    std::mem::forget(Session::start(pad.clone(), dir.path()).unwrap());
    assert_ne!(pad.modes(), before);
    restore_from_file(&mut pad, dir.path()).unwrap();
    assert_eq!(pad.writes(), then_stop(&START_WRITES));
    assert_eq!(pad.modes(), before);
    assert!(!path.exists());
}

#[test]
fn rotary_grips_are_refused() {
    let (pad, dir, _) = setup();
    pad.state().features = ROTARY_GRIPS;
    let result = Session::start(pad.clone(), dir.path());
    assert!(matches!(result, Err(SessionError::NotLinearGrips)));
    assert!(no_device_writes(&pad));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}
