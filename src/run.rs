//! The `run` loop: a source thread queues 1 kHz motor samples, and the calling thread sends
//! them to the pad eight at a time, one packet per 8 ms.
use crate::hidraw::Transport;
use crate::pacer::Pacer;
use crate::protocol::{PACKET_PERIOD, SAMPLES_PER_PACKET};
use crate::session::{Session, SessionError};
use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 40 ms of motor samples; older ones are dropped so latency stays bounded.
const QUEUE_CAP: usize = 40;
/// Capture delivers five samples per 5 ms and we take eight per 8 ms, so sending starts only
/// with two packets queued; taking from a nearly empty queue would cut holes in the carrier.
const PREFILL: usize = 16;
/// How long `run` waits for the source thread to finish after it is told to stop.
const SOURCE_EXIT_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub enum RunError {
    Source(io::Error),
    SourcePanicked,
    Session(SessionError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(e) => write!(f, "audio input failed: {e}"),
            Self::SourcePanicked => f.write_str("the audio input thread crashed"),
            Self::Session(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RunError {}

impl From<SessionError> for RunError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}

/// Motor samples between the source thread and the send loop.
struct SampleQueue {
    samples: VecDeque<[f32; 2]>,
    /// True until `PREFILL` samples are queued, and again after the queue runs empty.
    filling: bool,
}

impl SampleQueue {
    fn push(&mut self, block: &[[f32; 2]]) {
        self.samples.extend(block);
        let excess = self.samples.len().saturating_sub(QUEUE_CAP);
        self.samples.drain(..excess);
    }

    /// The next packet's samples. Missing ones are 0.0, which quantizes to neutral.
    fn take_packet(&mut self) -> ([f32; SAMPLES_PER_PACKET], [f32; SAMPLES_PER_PACKET]) {
        let mut left = [0.0; SAMPLES_PER_PACKET];
        let mut right = [0.0; SAMPLES_PER_PACKET];
        if self.filling && self.samples.len() < PREFILL {
            return (left, right);
        }
        self.filling = false;
        for i in 0..SAMPLES_PER_PACKET {
            let Some([l, r]) = self.samples.pop_front() else {
                // Refill before resuming; resuming at once would run dry on every tick and pad
                // each packet, because samples arrive five at a time.
                self.filling = true;
                break;
            };
            left[i] = l;
            right[i] = r;
        }
        (left, right)
    }
}

type SharedQueue = Arc<Mutex<SampleQueue>>;

/// The queue holds plain samples, so a thread that panicked holding the lock left nothing
/// half-updated that matters.
fn lock(queue: &SharedQueue) -> MutexGuard<'_, SampleQueue> {
    queue.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Streams `samples` to the pad until `stop` is set, the source fails or ends, or a send fails.
/// Always stops the session; a failed stop is printed with the kept map path.
pub fn run<T: Transport>(
    mut session: Session<T>,
    samples: impl Iterator<Item = io::Result<Vec<[f32; 2]>>> + Send + 'static,
    stop: Arc<AtomicBool>,
) -> Result<(), RunError> {
    let queue = Arc::new(Mutex::new(SampleQueue {
        samples: VecDeque::with_capacity(QUEUE_CAP),
        filling: true,
    }));
    let source = {
        let queue = Arc::clone(&queue);
        let stop = Arc::clone(&stop);
        thread::spawn(move || feed(samples, &queue, &stop))
    };
    let sent = send_loop(&mut session, &queue, &source, &stop);
    // Tells the source thread to quit too.
    stop.store(true, Ordering::Relaxed);
    let played = sent.map_err(RunError::from).and(source_result(source));
    session.stop_after(played)
}

/// Queues blocks until the source ends or `stop` is set. A source error after `stop` counts as
/// a clean stop: a signal to the capture's process group kills it too.
fn feed(
    samples: impl Iterator<Item = io::Result<Vec<[f32; 2]>>>,
    queue: &SharedQueue,
    stop: &AtomicBool,
) -> io::Result<()> {
    for block in samples {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        lock(queue).push(&block?);
    }
    if stop.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err(io::Error::other("the audio source ended"))
    }
}

/// Sends one packet per tick, neutral when the queue is empty, until `stop` is set or the source
/// thread finishes.
fn send_loop<T: Transport>(
    session: &mut Session<T>,
    queue: &SharedQueue,
    source: &JoinHandle<io::Result<()>>,
    stop: &AtomicBool,
) -> Result<(), SessionError> {
    let mut pacer = Pacer::new(PACKET_PERIOD);
    while !stop.load(Ordering::Relaxed) && !source.is_finished() {
        let (left, right) = lock(queue).take_packet();
        session.send(&left, &right)?;
        pacer.wait();
    }
    Ok(())
}

/// Waits up to `SOURCE_EXIT_WAIT` for the source thread. One still blocked in a read after that
/// is left behind: the pad matters more than the thread, and process exit ends it.
fn source_result(source: JoinHandle<io::Result<()>>) -> Result<(), RunError> {
    let deadline = Instant::now() + SOURCE_EXIT_WAIT;
    while !source.is_finished() {
        if Instant::now() >= deadline {
            return Ok(());
        }
        thread::sleep(PACKET_PERIOD);
    }
    match source.join() {
        Ok(result) => result.map_err(RunError::Source),
        Err(_) => Err(RunError::SourcePanicked),
    }
}
