//! Cancellable execution for short-lived local processes.
//!
//! The module owns the ordering-sensitive lifecycle shared by AI harnesses and
//! custom commands: stdio setup, spawn, bounded draining, cancellation polling,
//! child or process-group termination, waiting, and output collection. Callers
//! choose one of the two termination policies through named constructors and
//! wrap the returned [`ProcessOutcome`] with their own request metadata.

use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TERMINATION_GRACE: Duration = Duration::from_millis(100);
const TERMINATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const OUTPUT_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_CAPTURED_OUTPUT_BYTES: usize = 1024 * 1024;
const TRUNCATED_OUTPUT_NOTICE_MAX_BYTES: usize = 128;

/// One configured process whose complete lifecycle is cancellation-aware.
pub(crate) struct CancellableProcess {
    command: Command,
    stdin: Option<String>,
    termination: TerminationPolicy,
}

#[derive(Clone, Copy)]
enum TerminationPolicy {
    Child,
    ProcessGroup,
}

/// Non-blocking, bounded stdout/stderr collection for a running child process.
struct ProcessOutputCollector {
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

impl CancellableProcess {
    /// Run one child and terminate only that child when cancelled.
    pub(crate) fn child(command: Command) -> Self {
        Self {
            command,
            stdin: None,
            termination: TerminationPolicy::Child,
        }
    }

    /// Run one child in a new process group and terminate the group when
    /// cancelled. This keeps descendants from outliving shell commands.
    pub(crate) fn in_process_group(command: Command) -> Self {
        Self {
            command,
            stdin: None,
            termination: TerminationPolicy::ProcessGroup,
        }
    }

    pub(crate) fn stdin(mut self, stdin: String) -> Self {
        self.stdin = Some(stdin);
        self
    }

    /// Run to completion or cancellation while continuously draining output.
    pub(crate) fn run(mut self, cancel: &Receiver<()>) -> io::Result<ProcessOutcome> {
        self.prepare_command();
        let termination = self.termination;
        let mut child = self.command.spawn()?;
        let mut output = ProcessOutputCollector::start(&mut child);

        if let Some(stdin) = self.stdin.take()
            && let Err(error) = write_child_stdin(&mut child, &stdin)
        {
            let _ = terminate(&mut child, termination);
            return Err(error);
        }

        loop {
            output.drain();

            if cancellation_requested(cancel) {
                let status = terminate(&mut child, termination)?;
                return Ok(ProcessOutcome::cancelled(
                    status.map(|status| output.collect(status)),
                ));
            }

            if let Some(status) = child.try_wait()? {
                return Ok(ProcessOutcome::from_output(output.collect(status)));
            }

            thread::sleep(PROCESS_POLL_INTERVAL);
        }
    }

    fn prepare_command(&mut self) {
        if self.stdin.is_some() {
            self.command.stdin(Stdio::piped());
        } else {
            self.command.stdin(Stdio::null());
        }
        self.command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if matches!(self.termination, TerminationPolicy::ProcessGroup) {
            prepare_process_group(&mut self.command);
        }
    }
}

impl ProcessOutputCollector {
    fn start(child: &mut Child) -> Self {
        Self {
            stdout: child.stdout.take().map(spawn_bounded_reader),
            stderr: child.stderr.take().map(spawn_bounded_reader),
        }
    }

    fn drain(&mut self) {
        if let Some(stdout) = &mut self.stdout {
            stdout.drain();
        }
        if let Some(stderr) = &mut self.stderr {
            stderr.drain();
        }
    }

    fn collect(&mut self, status: ExitStatus) -> Output {
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

fn write_child_stdin(child: &mut Child, stdin: &str) -> io::Result<()> {
    let Some(mut child_stdin) = child.stdin.take() else {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "child stdin was unavailable",
        ));
    };
    match child_stdin.write_all(stdin.as_bytes()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

fn cancellation_requested(cancel: &Receiver<()>) -> bool {
    match cancel.try_recv() {
        Ok(()) | Err(TryRecvError::Disconnected) => true,
        Err(TryRecvError::Empty) => false,
    }
}

fn terminate(child: &mut Child, termination: TerminationPolicy) -> io::Result<Option<ExitStatus>> {
    match termination {
        TerminationPolicy::Child => {
            let _ = child.kill();
            Ok(child.wait().ok())
        }
        TerminationPolicy::ProcessGroup => terminate_process_group(child),
    }
}

fn terminate_process_group(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    terminate_process_group_gracefully(child);
    if let Some(status) = wait_for_exit(child, TERMINATION_GRACE)? {
        return Ok(Some(status));
    }

    terminate_process_group_forcefully(child);
    wait_for_exit(child, TERMINATION_GRACE)
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(TERMINATION_POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn prepare_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn prepare_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_group_gracefully(child: &mut Child) {
    let group = format!("-{}", child.id());
    let _ = Command::new("kill").args(["-TERM", &group]).status();
}

#[cfg(not(unix))]
fn terminate_process_group_gracefully(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
fn terminate_process_group_forcefully(child: &mut Child) {
    let group = format!("-{}", child.id());
    let _ = Command::new("kill").args(["-KILL", &group]).status();
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate_process_group_forcefully(child: &mut Child) {
    let _ = child.kill();
}

/// Captured output and completion status of one external process run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessOutcome {
    stdout: String,
    stderr: String,
    status: ProcessStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcessStatus {
    Completed { success: bool, code: Option<i32> },
    Cancelled { code: Option<i32> },
    NotStarted { error: String },
}

impl ProcessOutcome {
    /// A process that ran to completion, successful or not.
    pub(crate) fn from_output(output: Output) -> Self {
        let (stdout, stderr, status) = decode_output(output);
        Self {
            stdout,
            stderr,
            status: ProcessStatus::Completed {
                success: status.success(),
                code: status.code(),
            },
        }
    }

    /// A successful synthetic result for work that intentionally did not spawn.
    pub(crate) fn successful_stdout(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            status: ProcessStatus::Completed {
                success: true,
                code: Some(0),
            },
        }
    }

    /// A process killed on the caller's request, keeping whatever partial output
    /// it had produced.
    pub(crate) fn cancelled(output: Option<Output>) -> Self {
        let (stdout, stderr, code) = match output {
            Some(output) => {
                let (stdout, stderr, status) = decode_output(output);
                (stdout, stderr, status.code())
            }
            None => (String::new(), String::new(), None),
        };

        Self {
            stdout,
            stderr,
            status: ProcessStatus::Cancelled { code },
        }
    }

    /// A process that never started; the error is surfaced as stderr.
    pub(crate) fn not_started(error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            stdout: String::new(),
            stderr: error.clone(),
            status: ProcessStatus::NotStarted { error },
        }
    }

    pub(crate) fn stdout(&self) -> &str {
        &self.stdout
    }

    pub(crate) fn stderr(&self) -> &str {
        &self.stderr
    }

    pub(crate) fn success(&self) -> bool {
        matches!(&self.status, ProcessStatus::Completed { success: true, .. })
    }

    pub(crate) fn cancelled_status(&self) -> bool {
        matches!(&self.status, ProcessStatus::Cancelled { .. })
    }

    /// One-line, human-readable description of how the process ended.
    pub(crate) fn status_text(&self) -> String {
        match &self.status {
            ProcessStatus::Completed {
                code: Some(code), ..
            } => format!("exit {code}"),
            ProcessStatus::Completed { code: None, .. } => "terminated by signal".to_string(),
            ProcessStatus::Cancelled { code: _ } => "cancelled".to_string(),
            ProcessStatus::NotStarted { error } => format!("failed to start: {error}"),
        }
    }
}

fn decode_output(output: Output) -> (String, String, ExitStatus) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn process_group_cancellation_stops_descendants_and_keeps_partial_output() {
        let command = shell_command("printf start; sleep 5; printf done");
        let (cancel_sender, cancel) = mpsc::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel_sender.send(()).unwrap();
        });

        let started_at = Instant::now();
        let outcome = CancellableProcess::in_process_group(command)
            .run(&cancel)
            .unwrap();

        assert!(outcome.cancelled_status());
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(outcome.stdout().contains("start"));
        assert!(!outcome.stdout().contains("done"));
    }

    #[cfg(unix)]
    #[test]
    fn child_cancellation_terminates_and_collects_the_child() {
        let command = shell_command("printf start; exec sleep 5");
        let (cancel_sender, cancel) = mpsc::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel_sender.send(()).unwrap();
        });

        let outcome = CancellableProcess::child(command).run(&cancel).unwrap();

        assert!(outcome.cancelled_status());
        assert!(outcome.stdout().contains("start"));
    }

    #[cfg(unix)]
    #[test]
    fn running_process_drains_and_bounds_large_output() {
        let mut command = Command::new("head");
        command.args(["-c", "2097152", "/dev/zero"]);
        let (_cancel_sender, cancel) = mpsc::channel();

        let outcome = CancellableProcess::child(command).run(&cancel).unwrap();

        assert!(outcome.success());
        assert!(outcome.stdout().contains("output truncated"));
        assert!(
            outcome.stdout().len() <= MAX_CAPTURED_OUTPUT_BYTES + TRUNCATED_OUTPUT_NOTICE_MAX_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn completed_process_returns_when_descendant_keeps_stdout_open() {
        let command = shell_command("printf done; sleep 5 &");
        let (_cancel_sender, cancel) = mpsc::channel();

        let started_at = Instant::now();
        let outcome = CancellableProcess::in_process_group(command)
            .run(&cancel)
            .unwrap();

        assert_eq!(outcome.status_text(), "exit 0");
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(outcome.stdout().contains("done"));
    }

    #[cfg(unix)]
    #[test]
    fn process_input_is_written_and_closed_before_polling() {
        let command = shell_command("cat");
        let (_cancel_sender, cancel) = mpsc::channel();

        let outcome = CancellableProcess::child(command)
            .stdin("question\n".to_string())
            .run(&cancel)
            .unwrap();

        assert!(outcome.success());
        assert_eq!(outcome.stdout(), "question\n");
    }

    #[cfg(unix)]
    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }
}
