//! hidraw transport for the Apex 6 vendor interface, plus request/reply matching.
use crate::protocol::{self, FRAME_LEN, ProtocolError};
use nix::poll::{PollFd, PollFlags, PollTimeout};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Flydigi's vendor id and the Apex 6 Pro's product id, as the kernel prints them in `uevent`.
const HID_ID: &str = "HID_ID=0003:000037D7:00002502";
/// The vendor interface's report descriptor starts with usage page 0xFFA0 (`06 A0 FF`).
const VENDOR_USAGE_PAGE: [u8; 3] = [0x06, 0xA0, 0xFF];
/// Read buffer size; the engine uses at least 0x40 (note, item 1, 0x1402825f7).
const READ_BUFFER_LEN: usize = 64;
const UDEV_RULE: &str =
    "KERNEL==\"hidraw*\", ATTRS{idVendor}==\"37d7\", ATTRS{idProduct}==\"2502\", TAG+=\"uaccess\"";

pub trait Transport {
    fn write(&mut self, frame: &[u8; FRAME_LEN]) -> io::Result<()>;
    /// Returns the whole input report, or `None` if nothing arrived within `timeout`.
    fn read(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>>;
}

pub struct Hidraw {
    file: File,
}

impl Hidraw {
    pub fn open(path: &Path) -> io::Result<Hidraw> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| match e.kind() {
                ErrorKind::PermissionDenied => io::Error::new(
                    ErrorKind::PermissionDenied,
                    format!(
                        "cannot open {}: permission denied. Add \
                         /etc/udev/rules.d/70-open-haptics.rules with {UDEV_RULE}, \
                         and don't use sudo (the saved motor map would go to root's home)",
                        path.display()
                    ),
                ),
                _ => e,
            })?;
        Ok(Hidraw::from_file(file))
    }

    /// Wraps any pollable fd; the tests use a socket pair instead of a device node.
    pub fn from_file(file: File) -> Hidraw {
        Hidraw { file }
    }
}

impl Transport for Hidraw {
    /// Sends report ID `00` plus the frame (note, "Frame format", 0x1402882d0).
    fn write(&mut self, frame: &[u8; FRAME_LEN]) -> io::Result<()> {
        let mut report = [0u8; FRAME_LEN + 1];
        report[1..].copy_from_slice(frame);
        self.file.write_all(&report)
    }

    fn read(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let timeout = PollTimeout::try_from(timeout).unwrap_or(PollTimeout::MAX);
        let mut fds = [PollFd::new(self.file.as_fd(), PollFlags::POLLIN)];
        if nix::poll::poll(&mut fds, timeout)? == 0 {
            return Ok(None);
        }
        let mut buf = [0u8; READ_BUFFER_LEN];
        let n = self.file.read(&mut buf)?;
        Ok(Some(buf[..n].to_vec()))
    }
}

/// Finds the Apex 6 vendor interface under `/sys/class/hidraw`.
pub fn find_node(sysfs_class_hidraw: &Path) -> io::Result<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(sysfs_class_hidraw)? {
        let entry = entry?;
        let device = entry.path().join("device");
        let Ok(uevent) = fs::read_to_string(device.join("uevent")) else {
            continue;
        };
        let Ok(descriptor) = fs::read(device.join("report_descriptor")) else {
            continue;
        };
        if uevent.lines().any(|line| line == HID_ID) && descriptor.starts_with(&VENDOR_USAGE_PAGE) {
            found.push(PathBuf::from("/dev").join(entry.file_name()));
        }
    }
    match found.as_slice() {
        [node] => Ok(node.clone()),
        [] => Err(io::Error::new(
            ErrorKind::NotFound,
            "no Apex 6 vendor interface (usage page 0xFFA0) found; is the pad plugged in over USB?",
        )),
        _ => Err(io::Error::other(format!(
            "found {} Apex 6 vendor interfaces ({}); pick one with --device",
            found.len(),
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

#[derive(Debug)]
pub enum AskError {
    Io(io::Error),
    Protocol(ProtocolError),
    Timeout { cmd: u8 },
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "hidraw I/O failed: {e}"),
            Self::Protocol(e) => write!(f, "bad reply: {e}"),
            Self::Timeout { cmd } => write!(f, "no reply to command {cmd:#04x} before the timeout"),
        }
    }
}

impl std::error::Error for AskError {}

impl From<io::Error> for AskError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ProtocolError> for AskError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

/// Sends a request and returns the reply payload for the same command, dropping other reports
/// as the engine does (note, item 1, 0x1402829d9).
pub fn ask<T: Transport>(
    t: &mut T,
    cmd: u8,
    payload: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, AskError> {
    t.write(&protocol::frame(cmd, payload)?)?;
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AskError::Timeout { cmd });
        }
        let Some(report) = t.read(remaining)? else {
            return Err(AskError::Timeout { cmd });
        };
        match protocol::parse_reply(cmd, &report) {
            Ok(reply) => return Ok(reply.to_vec()),
            Err(ProtocolError::WrongCommand { .. } | ProtocolError::BadMagic) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Layouts copied from what I saw on 2026-11-04.
    const PAD_ID: &str = "HID_ID=0003:000037D7:00002502";
    const DONGLE_ID: &str = "HID_ID=0003:000037D7:00006001";
    const KEYBOARD: [u8; 4] = [0x05, 0x01, 0x09, 0x06];
    const VENDOR: [u8; 4] = [0x06, 0xA0, 0xFF, 0x09];
    const OTHER_VENDOR: [u8; 4] = [0x06, 0xEE, 0xFF, 0x09];

    fn fake_sysfs(nodes: &[(&str, &str, [u8; 4])]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, id, descriptor) in nodes {
            let device = dir.path().join(name).join("device");
            fs::create_dir_all(&device).unwrap();
            fs::write(device.join("uevent"), format!("DRIVER=hid-generic\n{id}\n")).unwrap();
            fs::write(device.join("report_descriptor"), descriptor).unwrap();
        }
        dir
    }

    fn sysfs_like_my_pc() -> TempDir {
        fake_sysfs(&[
            ("hidraw3", PAD_ID, KEYBOARD),
            ("hidraw12", PAD_ID, VENDOR),
            ("hidraw13", PAD_ID, OTHER_VENDOR),
            ("hidraw14", DONGLE_ID, VENDOR),
        ])
    }

    #[test]
    fn picks_the_apex6_vendor_node() {
        let dir = sysfs_like_my_pc();
        assert_eq!(
            find_node(dir.path()).unwrap(),
            PathBuf::from("/dev/hidraw12")
        );
    }

    #[test]
    fn no_pad_is_a_clear_error() {
        let dir = fake_sysfs(&[
            ("hidraw14", DONGLE_ID, VENDOR),
            ("hidraw3", PAD_ID, KEYBOARD),
        ]);
        let err = find_node(dir.path()).unwrap_err();
        assert!(
            err.to_string()
                .contains("no Apex 6 vendor interface (usage page 0xFFA0)")
        );
    }

    #[test]
    fn two_pads_ask_for_device() {
        let dir = fake_sysfs(&[("hidraw4", PAD_ID, VENDOR), ("hidraw12", PAD_ID, VENDOR)]);
        assert!(
            find_node(dir.path())
                .unwrap_err()
                .to_string()
                .contains("--device")
        );
    }
}
