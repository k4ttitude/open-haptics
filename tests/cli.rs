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
