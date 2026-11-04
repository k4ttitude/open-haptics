use open_haptics::pacer::Pacer;
use std::thread::sleep;
use std::time::{Duration, Instant};

const PERIOD: Duration = Duration::from_millis(8);

fn timed(f: impl FnOnce()) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

#[test]
fn ticks_every_period() {
    let mut pacer = Pacer::new(PERIOD);
    let elapsed = timed(|| {
        for _ in 0..25 {
            pacer.wait();
        }
    });
    // 25 waits at 8 ms are 200 ms. Deadlines do not drift, so only the last wake-up adds error.
    assert!(
        elapsed >= Duration::from_millis(198) && elapsed <= Duration::from_millis(230),
        "25 waits took {elapsed:?}"
    );
}

#[test]
fn stall_does_not_burst() {
    let mut pacer = Pacer::new(PERIOD);
    pacer.wait();
    sleep(Duration::from_millis(50));
    // The first wait after the stall may return at once. The next two must each take a period.
    pacer.wait();
    for _ in 0..2 {
        let elapsed = timed(|| pacer.wait());
        assert!(elapsed >= Duration::from_millis(6), "burst: {elapsed:?}");
    }
}
