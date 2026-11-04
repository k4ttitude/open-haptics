use std::process::Command;

#[test]
fn restore_needs_experimental() {
    // Without --experimental the binary exits before it looks for a device.
    let out = Command::new(env!("CARGO_BIN_EXE_open-haptics"))
        .args(["--device", "/nonexistent/hidraw", "restore"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--experimental"));
}

#[test]
fn tone_needs_experimental() {
    let out = Command::new(env!("CARGO_BIN_EXE_open-haptics"))
        .args(["--device", "/nonexistent/hidraw", "tone"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--experimental"));
}

#[test]
fn run_needs_experimental() {
    let out = Command::new(env!("CARGO_BIN_EXE_open-haptics"))
        .args(["--device", "/nonexistent/hidraw", "run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--experimental"));
}

#[test]
fn run_rejects_gain_out_of_range() {
    // The gain check comes before the capture starts or any device is opened.
    for gain in ["4.5", "-1", "NaN", "inf"] {
        let out = Command::new(env!("CARGO_BIN_EXE_open-haptics"))
            .args(["--device", "/nonexistent/hidraw", "--experimental", "run"])
            .arg(format!("--gain={gain}"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "--gain {gain}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("--gain must be a number from 0 to 4"),
            "--gain {gain}"
        );
    }
}
