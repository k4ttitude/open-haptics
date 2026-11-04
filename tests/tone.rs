mod common;

use common::fake_pad::FakePad;
use open_haptics::protocol::{
    CMD_MOTOR_STREAM, NEUTRAL, PACKET_PERIOD, REQUEST_PAYLOAD_START, SAMPLES_PER_PACKET,
    motor_packet,
};
use open_haptics::session::{Session, SessionError, finish};
use open_haptics::tone::{play, tick_samples};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[test]
fn tick_samples_are_continuous_sines() {
    let (left, right) = tick_samples(0);
    assert_eq!((left[0], right[0]), (0.0, 0.0));
    // 80 Hz at 1 kHz peaks at sample 3.125 and 120 Hz at 2.083, so both are near 0.15 by sample 3.
    assert!(left[3] > 0.14 && right[2] > 0.14);
    // The next tick continues the same wave rather than restarting it.
    let (next, _) = tick_samples(SAMPLES_PER_PACKET as u64);
    let (whole, _) = tick_samples(0);
    assert_ne!(next[0], whole[0]);
    assert!((next[0] - 0.15 * (std::f64::consts::TAU * 0.08 * 8.0).sin() as f32).abs() < 1e-6);
}

#[test]
fn tone_has_no_phase_drift_after_an_hour() {
    // 80 Hz is exactly 80 cycles per 1000 samples, so the wave repeats every second.
    let (start, _) = tick_samples(0);
    let (later, _) = tick_samples(3_600_000);
    assert_eq!(start, later);
}

#[test]
fn play_sends_grip_samples_with_neutral_triggers() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start(pad.clone(), dir.path()).unwrap();
    let ticks = 5;
    play(&mut session, PACKET_PERIOD * ticks, &AtomicBool::new(false)).unwrap();
    session.stop().unwrap();

    let frames: Vec<_> = pad
        .state()
        .log
        .iter()
        .filter(|f| f[2] == CMD_MOTOR_STREAM)
        .copied()
        .collect();
    assert_eq!(frames.len(), ticks as usize);
    for (tick, frame) in frames.iter().enumerate() {
        let (left, right) = tick_samples((tick * SAMPLES_PER_PACKET) as u64);
        assert_eq!(*frame, motor_packet(&left, &right));
        // The control byte comes first, then (trigger, left, right) triples.
        let triples = &frame[REQUEST_PAYLOAD_START + 1..][..3 * SAMPLES_PER_PACKET];
        for triple in triples.chunks(3) {
            assert_eq!(triple[0], NEUTRAL);
        }
    }
    assert!(
        frames
            .iter()
            .any(|f| f[REQUEST_PAYLOAD_START + 2] != NEUTRAL)
    );
}

#[test]
fn play_stops_when_flagged() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start(pad.clone(), dir.path()).unwrap();
    play(
        &mut session,
        Duration::from_secs(60),
        &AtomicBool::new(true),
    )
    .unwrap();
    assert_eq!(pad.count(CMD_MOTOR_STREAM), 0);
    session.stop().unwrap();
}

fn failure(text: &str) -> Result<(), SessionError> {
    Err(SessionError::Io(std::io::Error::other(text.to_string())))
}

fn finish_with(
    played: Result<(), SessionError>,
    stopped: Result<(), SessionError>,
) -> (Option<String>, Result<(), SessionError>) {
    finish(played, stopped, Path::new("/state/motor-map-x.bin"))
}

#[test]
fn both_failing_reports_the_stop_failure_and_returns_the_send_error() {
    let (message, outcome) = finish_with(failure("send"), failure("stop"));
    let message = message.expect("stop failure is reported");
    assert!(message.contains("stop"));
    assert!(message.contains("/state/motor-map-x.bin"));
    assert!(message.contains("open-haptics restore --experimental"));
    assert!(outcome.unwrap_err().to_string().contains("send"));
}

#[test]
fn send_failure_alone_has_no_stop_message() {
    let (message, outcome) = finish_with(failure("send"), Ok(()));
    assert!(message.is_none());
    assert!(outcome.unwrap_err().to_string().contains("send"));
}

#[test]
fn stop_failure_alone_is_reported_and_returned() {
    let (message, outcome) = finish_with(Ok(()), failure("stop"));
    assert!(
        message
            .expect("reported")
            .contains("restore --experimental")
    );
    assert!(outcome.unwrap_err().to_string().contains("stop"));
}

#[test]
fn clean_play_and_stop_is_ok() {
    let (message, outcome) = finish_with(Ok(()), Ok(()));
    assert!(message.is_none());
    assert!(outcome.is_ok());
}
