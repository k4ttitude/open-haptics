use open_haptics::hidraw::{Hidraw, Transport};
use open_haptics::protocol::{CMD_INFO, frame};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// A socket pair stands in for the device node; `Hidraw` only needs a pollable fd.
fn pair() -> (Hidraw, UnixStream) {
    let (ours, peer) = UnixStream::pair().unwrap();
    (
        Hidraw::from_file(File::from(std::os::fd::OwnedFd::from(ours))),
        peer,
    )
}

#[test]
fn read_returns_none_when_nothing_arrives() {
    let (mut pad, _peer) = pair();
    assert_eq!(pad.read(Duration::from_millis(20)).unwrap(), None);
}

#[test]
fn read_returns_the_bytes_that_arrived() {
    let (mut pad, mut peer) = pair();
    peer.write_all(&[1, 2, 3]).unwrap();
    assert_eq!(
        pad.read(Duration::from_millis(500)).unwrap(),
        Some(vec![1, 2, 3])
    );
}

#[test]
fn write_sends_report_id_zero_then_the_frame() {
    let (mut pad, mut peer) = pair();
    let frame = frame(CMD_INFO, &[]).unwrap();
    pad.write(&frame).unwrap();
    let mut got = [0u8; 33];
    peer.read_exact(&mut got).unwrap();
    assert_eq!(got[0], 0);
    assert_eq!(&got[1..], &frame);
}
