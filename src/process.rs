//! The outcome of running an external process in the background.
//!
//! `ask_ai` and `custom_command` both spawn a child process and report the same
//! shape of result: captured stdout/stderr plus a status that is either a
//! completed exit code, a cancellation, or a failure to start. This module owns
//! that shape and its status-text formatting once. Each caller wraps a
//! `ProcessOutcome` alongside its own request metadata, so the difference
//! between the two is only what each remembers about the request — never how a
//! process result is built or described.

use std::io::Read;
use std::process::{Child, ExitStatus, Output};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const OUTPUT_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(crate) const MAX_CAPTURED_OUTPUT_BYTES: usize = 1024 * 1024;
pub(crate) const TRUNCATED_OUTPUT_NOTICE_MAX_BYTES: usize = 128;

/// Non-blocking, bounded stdout/stderr collection for a running child process.
pub(crate) struct ProcessOutputCollector {
    stdout: Option<StreamCollector>,
    stderr: Option<StreamCollector>,
}

#[derive(Default)]
struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

struct StreamCollector {
    events: Receiver<StreamEvent>,
    captured: CapturedStream,
    finished: bool,
}

enum StreamEvent {
    Chunk(Vec<u8>),
    Truncated,
}

impl ProcessOutputCollector {
    pub(crate) fn start(child: &mut Child) -> Self {
        Self {
            stdout: child.stdout.take().map(spawn_bounded_reader),
            stderr: child.stderr.take().map(spawn_bounded_reader),
        }
    }

    pub(crate) fn drain(&mut self) {
        if let Some(stdout) = &mut self.stdout {
            stdout.drain();
        }
        if let Some(stderr) = &mut self.stderr {
            stderr.drain();
        }
    }

    pub(crate) fn collect(&mut self, status: ExitStatus) -> Output {
        self.drain_until_finished_or_timeout();

        Output {
            status,
            stdout: take_captured_stream(&mut self.stdout),
            stderr: take_captured_stream(&mut self.stderr),
        }
    }

    fn drain_until_finished_or_timeout(&mut self) {
        let deadline = Instant::now() + OUTPUT_DRAIN_GRACE;

        while !self.collectors_finished() && Instant::now() < deadline {
            let stdout_received = self.stdout.as_mut().is_some_and(StreamCollector::drain);
            let stderr_received = self.stderr.as_mut().is_some_and(StreamCollector::drain);
            if !stdout_received && !stderr_received {
                thread::sleep(OUTPUT_DRAIN_POLL_INTERVAL);
            }
        }
    }

    fn collectors_finished(&self) -> bool {
        self.stdout.as_ref().is_none_or(|stream| stream.finished)
            && self.stderr.as_ref().is_none_or(|stream| stream.finished)
    }
}

impl StreamCollector {
    fn drain(&mut self) -> bool {
        let mut received = false;

        loop {
            match self.events.try_recv() {
                Ok(StreamEvent::Chunk(bytes)) => {
                    self.captured.bytes.extend_from_slice(&bytes);
                    received = true;
                }
                Ok(StreamEvent::Truncated) => {
                    self.captured.truncated = true;
                    received = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }

        received
    }
}

impl CapturedStream {
    fn into_bytes(mut self) -> Vec<u8> {
        if self.truncated {
            let notice = truncated_output_notice();
            debug_assert!(notice.len() <= TRUNCATED_OUTPUT_NOTICE_MAX_BYTES);
            self.bytes.extend_from_slice(&notice);
        }
        self.bytes
    }
}

fn spawn_bounded_reader(mut stream: impl Read + Send + 'static) -> StreamCollector {
    let (sender, events) = mpsc::channel();
    thread::spawn(move || read_bounded_stream(&mut stream, sender));

    StreamCollector {
        events,
        captured: CapturedStream::default(),
        finished: false,
    }
}

fn read_bounded_stream(stream: &mut impl Read, sender: mpsc::Sender<StreamEvent>) {
    let mut captured_len = 0usize;
    let mut truncated = false;
    let mut buffer = [0; 8192];

    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(captured_len);
        if remaining > 0 {
            let captured = read.min(remaining);
            if sender
                .send(StreamEvent::Chunk(buffer[..captured].to_vec()))
                .is_err()
            {
                break;
            }
            captured_len += captured;
        }
        if read > remaining && !truncated {
            truncated = true;
            if sender.send(StreamEvent::Truncated).is_err() {
                break;
            }
        }
    }
}

fn take_captured_stream(collector: &mut Option<StreamCollector>) -> Vec<u8> {
    collector
        .take()
        .map(|collector| collector.captured.into_bytes())
        .unwrap_or_default()
}

fn truncated_output_notice() -> Vec<u8> {
    format!("\n[chunk: output truncated after {MAX_CAPTURED_OUTPUT_BYTES} bytes]\n").into_bytes()
}

/// Captured output and completion status of one external process run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessOutcome {
    stdout: String,
    stderr: String,
    status: ProcessStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessStatus {
    success: bool,
    code: Option<i32>,
    cancelled: bool,
    start_error: Option<String>,
}

impl ProcessOutcome {
    /// A process that ran to completion, successful or not.
    pub(crate) fn from_output(output: Output) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: ProcessStatus {
                success: output.status.success(),
                code: output.status.code(),
                cancelled: false,
                start_error: None,
            },
        }
    }

    /// A successful synthetic result for work that intentionally did not spawn.
    pub(crate) fn successful_stdout(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            status: ProcessStatus {
                success: true,
                code: Some(0),
                cancelled: false,
                start_error: None,
            },
        }
    }

    /// A process killed on the caller's request, keeping whatever partial output
    /// it had produced.
    pub(crate) fn cancelled(output: Option<Output>) -> Self {
        let (stdout, stderr, code) = output.map_or_else(
            || (String::new(), String::new(), None),
            |output| {
                (
                    String::from_utf8_lossy(&output.stdout).to_string(),
                    String::from_utf8_lossy(&output.stderr).to_string(),
                    output.status.code(),
                )
            },
        );

        Self {
            stdout,
            stderr,
            status: ProcessStatus {
                success: false,
                code,
                cancelled: true,
                start_error: None,
            },
        }
    }

    /// A process that never started; the error is surfaced as stderr.
    pub(crate) fn not_started(error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            stdout: String::new(),
            stderr: error.clone(),
            status: ProcessStatus {
                success: false,
                code: None,
                cancelled: false,
                start_error: Some(error),
            },
        }
    }

    pub(crate) fn stdout(&self) -> &str {
        &self.stdout
    }

    pub(crate) fn stderr(&self) -> &str {
        &self.stderr
    }

    pub(crate) fn success(&self) -> bool {
        self.status.success
    }

    pub(crate) fn cancelled_status(&self) -> bool {
        self.status.cancelled
    }

    /// One-line, human-readable description of how the process ended.
    pub(crate) fn status_text(&self) -> String {
        if self.status.cancelled {
            return "cancelled".to_string();
        }
        if let Some(error) = &self.status.start_error {
            return format!("failed to start: {error}");
        }

        match self.status.code {
            Some(code) => format!("exit {code}"),
            None => "terminated by signal".to_string(),
        }
    }
}
