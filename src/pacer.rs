//! Fixed-rate tick for the send loop.
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct Pacer {
    period: Duration,
    deadline: Instant,
}

impl Pacer {
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            deadline: Instant::now() + period,
        }
    }

    /// Sleeps until the next deadline. Deadlines are exact multiples of the period, so sleep
    /// overshoot does not accumulate. After a stall (the deadline already passed on entry) the
    /// next deadline is one period from now, so the loop does not burst to catch up.
    pub fn wait(&mut self) {
        let now = Instant::now();
        if now >= self.deadline {
            self.deadline = now + self.period;
        } else {
            sleep(self.deadline - now);
            self.deadline += self.period;
        }
    }
}
