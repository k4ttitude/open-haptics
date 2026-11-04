mod common;

use common::{ScriptedTransport, reply};
use open_haptics::hidraw::{AskError, Transport, ask};
use open_haptics::protocol::{CMD_INFO, CMD_MODE, CMD_MOTOR_STREAM};
use std::io;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_millis(0x5dc);

#[test]
fn ask_skips_unrelated_replies_and_times_out() {
    let mut t = ScriptedTransport::new(vec![
        reply(CMD_MOTOR_STREAM, &[1, 2, 3]),
        vec![0; 64],
        reply(CMD_MODE, &[0]),
        reply(CMD_INFO, &[9, 8, 7]),
    ]);
    let payload = ask(&mut t, CMD_INFO, &[], TIMEOUT).unwrap();
    assert_eq!(&payload[..3], &[9, 8, 7]);
    assert_eq!(t.written.len(), 1);
    assert_eq!(t.written[0][2], CMD_INFO);

    let mut silent = ScriptedTransport::new(vec![reply(CMD_MODE, &[0])]);
    assert!(matches!(
        ask(&mut silent, CMD_INFO, &[], TIMEOUT),
        Err(AskError::Timeout { cmd: CMD_INFO })
    ));
}

#[test]
fn ask_stops_at_the_deadline_when_only_unrelated_reports_keep_coming() {
    // Always-ready transport that never sends the right reply.
    struct Chatty;
    impl Transport for Chatty {
        fn write(&mut self, _: &[u8; 32]) -> io::Result<()> {
            Ok(())
        }
        fn read(&mut self, _: Duration) -> io::Result<Option<Vec<u8>>> {
            std::thread::sleep(Duration::from_millis(5));
            Ok(Some(reply(CMD_MODE, &[0])))
        }
    }
    let started = Instant::now();
    let result = ask(&mut Chatty, CMD_INFO, &[], Duration::from_millis(50));
    assert!(matches!(result, Err(AskError::Timeout { cmd: CMD_INFO })));
    assert!(started.elapsed() < Duration::from_millis(500));
}
