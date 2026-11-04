use open_haptics::capture::{BLOCK_FRAMES, Capture};
use std::path::Path;

#[test]
fn reassembles_seven_byte_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frames.f32");
    let mut bytes = Vec::new();
    for i in 0..2 * BLOCK_FRAMES {
        bytes.extend_from_slice(&(i as f32).to_le_bytes());
        bytes.extend_from_slice(&(-(i as f32)).to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    let input = format!("if={}", path.display());

    // dd writes 7 bytes at a time, so frames arrive split across reads.
    let mut capture = Capture::spawn(&["dd", &input, "bs=7", "status=none"]).unwrap();
    let first = capture.next().unwrap().unwrap();
    let second = capture.next().unwrap().unwrap();
    assert_eq!(first.len(), BLOCK_FRAMES);
    assert_eq!(first[1], [1.0, -1.0]);
    assert_eq!(second[0], [240.0, -240.0]);
    assert_eq!(second[BLOCK_FRAMES - 1], [479.0, -479.0]);
    // dd exits 0 at the end of the file; capture still reports that it ended.
    assert!(capture.next().unwrap().is_err());
    assert!(capture.next().is_none());
}

#[test]
fn child_failure_reports_code_and_stderr() {
    let mut capture = Capture::spawn(&["sh", "-c", "echo no sink >&2; exit 3"]).unwrap();
    let message = capture.next().unwrap().unwrap_err().to_string();
    assert!(message.contains("exit status: 3"), "{message}");
    assert!(message.contains("no sink"), "{message}");
}

#[test]
fn drop_kills_the_child() {
    let mut capture = Capture::spawn(&["sh", "-c", "exec cat /dev/zero"]).unwrap();
    let pid = capture.pid();
    assert_eq!(capture.next().unwrap().unwrap().len(), BLOCK_FRAMES);
    drop(capture);
    // Reaped, so not even a zombie entry is left.
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn missing_program_fails_to_spawn() {
    assert!(Capture::spawn(&["open-haptics-no-such-program"]).is_err());
    assert!(Capture::spawn(&[]).is_err());
}
