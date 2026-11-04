//! The `tone` test signal: a fixed sine per grip, sent at the motor rate.
use crate::hidraw::Transport;
use crate::pacer::Pacer;
use crate::protocol::{MOTOR_RATE_HZ, SAMPLES_PER_PACKET};
use crate::session::{Session, SessionError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const LEFT_HZ: f64 = 80.0;
const RIGHT_HZ: f64 = 120.0;
/// Low on purpose: this is the first signal ever sent to the real pad.
const AMPLITUDE: f32 = 0.15;
/// 8 ms: one `0x57` packet of eight samples at the motor rate.
pub const PACKET_PERIOD: Duration =
    Duration::from_millis(SAMPLES_PER_PACKET as u64 * 1000 / MOTOR_RATE_HZ);

/// Eight (left, right) samples starting at motor sample `first`. The phase comes from the sample
/// counter, not an accumulator, so it does not drift.
pub fn tick_samples(first: u64) -> ([f32; SAMPLES_PER_PACKET], [f32; SAMPLES_PER_PACKET]) {
    let mut left = [0.0; SAMPLES_PER_PACKET];
    let mut right = [0.0; SAMPLES_PER_PACKET];
    for i in 0..SAMPLES_PER_PACKET {
        let n = first + i as u64;
        left[i] = sample(LEFT_HZ, n);
        right[i] = sample(RIGHT_HZ, n);
    }
    (left, right)
}

fn sample(hz: f64, n: u64) -> f32 {
    // Reduce to a fraction of a cycle in f64 before scaling to radians, so large n stays exact.
    let cycles = (hz * n as f64 / MOTOR_RATE_HZ as f64).fract();
    AMPLITUDE * (std::f64::consts::TAU * cycles).sin() as f32
}

/// Sends the tone for `duration` or until `stop` is set, one packet per `PACKET_PERIOD`.
pub fn play<T: Transport>(
    session: &mut Session<T>,
    duration: Duration,
    stop: &AtomicBool,
) -> Result<(), SessionError> {
    let ticks = duration.as_nanos() / PACKET_PERIOD.as_nanos();
    let mut pacer = Pacer::new(PACKET_PERIOD);
    for tick in 0..ticks {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let (left, right) = tick_samples(tick as u64 * SAMPLES_PER_PACKET as u64);
        session.send(&left, &right)?;
        pacer.wait();
    }
    Ok(())
}
