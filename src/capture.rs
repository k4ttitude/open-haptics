//! System audio capture: a `pw-record` child writing raw 48 kHz stereo f32 to a pipe.
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::{self, JoinHandle};

// shortcut: pw-record child captures only the default sink; switch to the pipewire crate when per-game capture needs node discovery
/// `--raw` because without it pw-record writes a 24-byte Sun AU header first, even to a pipe
/// (checked with pw-record on libpipewire 1.6.9).
pub const PW_RECORD: &[&str] = &[
    "pw-record",
    "--raw",
    "-P",
    "stream.capture.sink=true",
    "--rate",
    "48000",
    "--channels",
    "2",
    "--format",
    "f32",
    "--latency",
    "5ms",
    "-",
];
/// 5 ms at 48 kHz: five mapper hops per block.
pub const BLOCK_FRAMES: usize = 240;
const FRAME_BYTES: usize = 2 * size_of::<f32>();
/// How much of the child's stderr an error quotes.
const STDERR_TAIL: usize = 500;

pub struct Capture {
    program: String,
    child: Child,
    stdout: ChildStdout,
    stderr: Option<JoinHandle<Vec<u8>>>,
    done: bool,
}

impl Capture {
    /// Starts `cmd` in its own process group, so a Ctrl+C in the terminal reaches only us.
    pub fn spawn(cmd: &[&str]) -> io::Result<Capture> {
        let (program, args) = cmd
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty capture command"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("cannot start {program}: {e}")))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(io::Error::other("capture child has no pipes"));
        };
        // Drained on a thread so a chatty child never blocks on a full stderr pipe.
        let stderr = thread::spawn(move || stderr_tail(stderr));
        Ok(Capture {
            program: (*program).to_owned(),
            child,
            stdout,
            stderr: Some(stderr),
            done: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Reads one whole block; `Ok(false)` means the stream ended first.
    fn read_block(&mut self, buf: &mut [u8]) -> io::Result<bool> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.stdout.read(&mut buf[filled..]) {
                Ok(0) => return Ok(false),
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }

    /// The error for a stream that ended: how the child exited and the end of its stderr.
    fn ended(&mut self) -> io::Error {
        let status = match self.child.wait() {
            Ok(status) => status.to_string(),
            Err(e) => format!("exit status unknown: {e}"),
        };
        let stderr = self
            .stderr
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default();
        io::Error::other(format!(
            "{} stopped ({status}): {}",
            self.program,
            String::from_utf8_lossy(&stderr).trim()
        ))
    }
}

impl Iterator for Capture {
    type Item = io::Result<Vec<[f32; 2]>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut buf = [0u8; BLOCK_FRAMES * FRAME_BYTES];
        let result = match self.read_block(&mut buf) {
            Ok(true) => return Some(Ok(frames(&buf))),
            Ok(false) => self.ended(),
            Err(e) => e,
        };
        self.done = true;
        Some(Err(result))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Errors only mean the child already exited and was reaped.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Little-endian f32 samples, interleaved left then right.
fn frames(bytes: &[u8]) -> Vec<[f32; 2]> {
    let samples: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    samples.as_chunks::<2>().0.to_vec()
}

/// Reads `stderr` to its end, keeping the last `STDERR_TAIL` bytes.
fn stderr_tail(mut stderr: impl Read) -> Vec<u8> {
    let mut tail = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match stderr.read(&mut buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Ok(0) | Err(_) => return tail,
            Ok(n) => {
                tail.extend_from_slice(&buf[..n]);
                let excess = tail.len().saturating_sub(STDERR_TAIL);
                tail.drain(..excess);
            }
        }
    }
}
