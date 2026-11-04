mod common;

use common::fake_pad::{FakePad, UUID_HEX};
use open_haptics::pacer::Pacer;
use open_haptics::protocol::{
    CMD_MOTOR_STREAM, NEUTRAL, REQUEST_PAYLOAD_START, SAMPLES_PER_PACKET,
};
use open_haptics::run::{RunError, run};
use open_haptics::session::{Session, map_path};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Capture delivers 240 audio frames every 5 ms, which the mapper turns into 5 motor samples.
const SAMPLES_PER_BLOCK: usize = 5;
const BLOCK_PERIOD: Duration = Duration::from_millis(5);
/// Quantizes to 0xBF, so it never looks like neutral padding.
const TONE: f32 = 0.5;

type Block = io::Result<Vec<[f32; 2]>>;

/// A source that yields `blocks` blocks of `value` on a 5 ms clock, like capture does. It can
/// pause once before block `stall.0`, recording when the pause ran, and it sets `stop_at_end`
/// when it runs out.
struct PacedSource {
    blocks: usize,
    value: f32,
    stall: Option<(usize, Duration)>,
    stall_window: Arc<Mutex<Option<(Instant, Instant)>>>,
    stop_at_end: Option<Arc<AtomicBool>>,
    pacer: Option<Pacer>,
    sent: usize,
}

impl PacedSource {
    fn new(blocks: usize, value: f32) -> Self {
        Self {
            blocks,
            value,
            stall: None,
            stall_window: Arc::default(),
            stop_at_end: None,
            pacer: None,
            sent: 0,
        }
    }

    fn stopping(mut self, stop: &Arc<AtomicBool>) -> Self {
        self.stop_at_end = Some(Arc::clone(stop));
        self
    }
}

impl Iterator for PacedSource {
    type Item = Block;

    fn next(&mut self) -> Option<Block> {
        if self.sent == self.blocks {
            if let Some(stop) = &self.stop_at_end {
                stop.store(true, Ordering::Relaxed);
            }
            return None;
        }
        if let Some((at, pause)) = self.stall
            && at == self.sent
        {
            let start = Instant::now();
            thread::sleep(pause);
            *self.stall_window.lock().unwrap() = Some((start, Instant::now()));
        }
        // Created on first use, in the thread that reads the source.
        self.pacer
            .get_or_insert_with(|| Pacer::new(BLOCK_PERIOD))
            .wait();
        self.sent += 1;
        Some(Ok(vec![[self.value; 2]; SAMPLES_PER_BLOCK]))
    }
}

/// The grip bytes (left, right per sample) of every `0x57` packet, with its send time.
fn motor_packets(pad: &FakePad) -> Vec<(Instant, Vec<u8>)> {
    let state = pad.state();
    let mut packets = Vec::new();
    for (frame, at) in state.log.iter().zip(&state.sent_at) {
        if frame[2] != CMD_MOTOR_STREAM {
            continue;
        }
        let triples = &frame[REQUEST_PAYLOAD_START + 1..][..3 * SAMPLES_PER_PACKET];
        let grips = triples.chunks(3).flat_map(|t| [t[1], t[2]]).collect();
        packets.push((*at, grips));
    }
    packets
}

fn is_neutral(grips: &[u8]) -> bool {
    grips.iter().all(|b| *b == NEUTRAL)
}

fn assert_restored(pad: &FakePad, dir: &Path) {
    assert_eq!(pad.modes(), FakePad::new().modes());
    assert!(!map_path(dir, UUID_HEX).exists());
}

fn run_with(
    pad: &FakePad,
    dir: &Path,
    samples: impl Iterator<Item = Block> + Send + 'static,
    stop: Arc<AtomicBool>,
) -> Result<(), RunError> {
    let session = Session::start(pad.clone(), dir).unwrap();
    run(session, samples, stop)
}

#[test]
fn silence_sends_only_neutral_packets() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let source = PacedSource::new(40, 0.0).stopping(&stop);
    run_with(&pad, dir.path(), source, stop).unwrap();

    let packets = motor_packets(&pad);
    // 200 ms at one packet per 8 ms is 25.
    assert!(packets.len() >= 20, "{} packets", packets.len());
    assert!(packets.iter().all(|(_, grips)| is_neutral(grips)));
    assert_restored(&pad, dir.path());
}

#[test]
fn steady_tone_has_no_padded_samples() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let source = PacedSource::new(100, TONE).stopping(&stop);
    run_with(&pad, dir.path(), source, stop).unwrap();

    let packets = motor_packets(&pad);
    let first_tone = packets
        .iter()
        .position(|(_, grips)| !is_neutral(grips))
        .expect("some tone was sent");
    let tone = &packets[first_tone..];
    // 500 ms of tone is 62 packets; the prefill and the end take a few.
    assert!(tone.len() >= 50, "{} tone packets", tone.len());
    for (i, (_, grips)) in tone.iter().enumerate() {
        assert!(!grips.contains(&NEUTRAL), "tone packet {i} is padded");
    }
}

#[test]
fn stall_sends_neutral_on_time() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let mut source = PacedSource::new(80, TONE).stopping(&stop);
    source.stall = Some((40, Duration::from_millis(100)));
    let window = Arc::clone(&source.stall_window);
    run_with(&pad, dir.path(), source, stop).unwrap();

    let (start, end) = window.lock().unwrap().expect("the source stalled");
    let packets = motor_packets(&pad);
    let in_gap: Vec<_> = packets
        .iter()
        .filter(|(at, _)| (start..end).contains(at))
        .collect();
    assert!(
        (10..=14).contains(&in_gap.len()),
        "{} packets in the 100 ms gap",
        in_gap.len()
    );
    // The queue holds at most 24 samples when the stall starts, three packets' worth.
    let drained = start + Duration::from_millis(25);
    for (at, grips) in &in_gap {
        if *at >= drained {
            assert!(is_neutral(grips), "packet in the gap carries samples");
        }
    }
    let after = packets
        .iter()
        .filter(|(at, _)| (end..end + Duration::from_millis(100)).contains(at))
        .count();
    assert!(after <= 14, "{after} packets in the 100 ms after the stall");
}

#[test]
fn source_error_restores_the_pad() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let source =
        PacedSource::new(10, TONE).chain(std::iter::once(Err(io::Error::other("capture died"))));
    let err = run_with(&pad, dir.path(), source, Arc::default()).unwrap_err();
    assert!(matches!(err, RunError::Source(_)));
    assert!(err.to_string().contains("capture died"), "{err}");
    assert_restored(&pad, dir.path());
}

#[test]
fn source_ending_is_an_error() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let err = run_with(&pad, dir.path(), PacedSource::new(3, TONE), Arc::default()).unwrap_err();
    assert!(matches!(err, RunError::Source(_)));
    assert_restored(&pad, dir.path());
}

#[test]
fn source_error_after_stop_is_a_clean_stop() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    // What a signal to the capture's process group looks like: the flag, then a dead child.
    let source = std::iter::from_fn(move || {
        flag.store(true, Ordering::Relaxed);
        Some(Err(io::Error::other("pw-record exited")))
    });
    run_with(&pad, dir.path(), source, stop).unwrap();
    assert_restored(&pad, dir.path());
}

#[test]
fn stop_flag_restores_the_pad() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let signal = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        flag.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    run_with(&pad, dir.path(), PacedSource::new(usize::MAX, TONE), stop).unwrap();
    signal.join().unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(pad.count(CMD_MOTOR_STREAM) > 0);
    assert_restored(&pad, dir.path());
}

#[test]
fn source_panic_restores_the_pad() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let source = std::iter::from_fn(|| -> Option<Block> { panic!("source bug") });
    let err = run_with(&pad, dir.path(), source, Arc::default()).unwrap_err();
    assert!(matches!(err, RunError::SourcePanicked), "{err}");
    assert_restored(&pad, dir.path());
}

#[test]
fn send_failure_keeps_the_saved_map() {
    let pad = FakePad::new();
    let dir = tempfile::tempdir().unwrap();
    let session = Session::start(pad.clone(), dir.path()).unwrap();
    // The pad unplugs after five motor packets, so the restore fails too.
    let written = pad.state().log.len();
    pad.state().disconnect_after_writes = Some(written + 5);
    let source = PacedSource::new(usize::MAX, TONE);
    let err = run(session, source, Arc::default()).unwrap_err();
    assert!(matches!(err, RunError::Session(_)), "{err}");
    assert!(map_path(dir.path(), UUID_HEX).exists());
}
