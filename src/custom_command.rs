//! User-configured shell command bindings and execution.

use std::env;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, eyre};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::process::ProcessOutcome;

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TERMINATION_GRACE: Duration = Duration::from_millis(100);
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const OUTPUT_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_CAPTURED_OUTPUT_BYTES: usize = 1024 * 1024;
const TRUNCATED_OUTPUT_NOTICE_MAX_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomCommandBinding {
    key: CommandKey,
    label: String,
    command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CommandKey {
    value: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomCommandResult {
    label: String,
    command: String,
    cwd: Option<PathBuf>,
    outcome: ProcessOutcome,
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

impl CustomCommandBinding {
    pub(crate) fn new(key: CommandKey, label: String, command: String) -> Self {
        Self {
            key,
            label,
            command,
        }
    }

    pub(crate) fn key(&self) -> CommandKey {
        self.key
    }

    pub(crate) fn key_display(&self) -> String {
        self.key.display()
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }
}

impl CommandKey {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        let mut chars = raw.chars();
        let Some(value) = chars.next() else {
            return Err(eyre!("custom command key cannot be empty"));
        };
        if chars.next().is_some() {
            return Err(eyre!(
                "custom command key `{raw}` must be a single character"
            ));
        }
        if value.is_control() {
            return Err(eyre!("custom command key cannot be a control character"));
        }

        Ok(Self { value })
    }

    pub(crate) fn char(self) -> char {
        self.value
    }

    pub(crate) fn matches(self, key: KeyEvent) -> bool {
        key.code == KeyCode::Char(self.value)
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }

    fn display(self) -> String {
        match self.value {
            ' ' => "Space".to_string(),
            value => value.to_string(),
        }
    }
}

impl CustomCommandResult {
    pub(crate) fn from_output(
        binding: &CustomCommandBinding,
        cwd: PathBuf,
        output: Output,
    ) -> Self {
        Self {
            label: binding.label.clone(),
            command: binding.command.clone(),
            cwd: Some(cwd),
            outcome: ProcessOutcome::from_output(output),
        }
    }

    pub(crate) fn not_started(
        binding: &CustomCommandBinding,
        cwd: Option<PathBuf>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            label: binding.label.clone(),
            command: binding.command.clone(),
            cwd,
            outcome: ProcessOutcome::not_started(error),
        }
    }

    pub(crate) fn cancelled(
        binding: &CustomCommandBinding,
        cwd: PathBuf,
        output: Option<Output>,
    ) -> Self {
        Self {
            label: binding.label.clone(),
            command: binding.command.clone(),
            cwd: Some(cwd),
            outcome: ProcessOutcome::cancelled(output),
        }
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    pub(crate) fn stdout(&self) -> &str {
        self.outcome.stdout()
    }

    pub(crate) fn stderr(&self) -> &str {
        self.outcome.stderr()
    }

    pub(crate) fn success(&self) -> bool {
        self.outcome.success()
    }

    pub(crate) fn status_text(&self) -> String {
        self.outcome.status_text()
    }
}

pub(crate) fn run(
    binding: &CustomCommandBinding,
    cwd: PathBuf,
    cancel: Receiver<()>,
) -> std::io::Result<CustomCommandResult> {
    let mut process = shell_command(binding.command());
    process
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_process(&mut process);

    let mut child = process.spawn()?;
    let mut stdout = child.stdout.take().map(spawn_bounded_reader);
    let mut stderr = child.stderr.take().map(spawn_bounded_reader);

    loop {
        drain_collectors(&mut stdout, &mut stderr);

        if cancellation_requested(&cancel) {
            let status = terminate_child(&mut child)?;
            let output = status.map(|status| collect_output(status, &mut stdout, &mut stderr));
            return Ok(CustomCommandResult::cancelled(binding, cwd, output));
        }

        if let Some(status) = child.try_wait()? {
            let output = collect_output(status, &mut stdout, &mut stderr);
            return Ok(CustomCommandResult::from_output(binding, cwd, output));
        }

        thread::sleep(COMMAND_POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn prepare_process(process: &mut Command) {
    use std::os::unix::process::CommandExt;

    process.process_group(0);
}

#[cfg(not(unix))]
fn prepare_process(_process: &mut Command) {}

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

fn collect_output(
    status: ExitStatus,
    stdout: &mut Option<StreamCollector>,
    stderr: &mut Option<StreamCollector>,
) -> Output {
    drain_collectors_until_finished_or_timeout(stdout, stderr);

    Output {
        status,
        stdout: take_captured_stream(stdout),
        stderr: take_captured_stream(stderr),
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

fn drain_collectors(stdout: &mut Option<StreamCollector>, stderr: &mut Option<StreamCollector>) {
    if let Some(stdout) = stdout {
        stdout.drain();
    }
    if let Some(stderr) = stderr {
        stderr.drain();
    }
}

fn drain_collectors_until_finished_or_timeout(
    stdout: &mut Option<StreamCollector>,
    stderr: &mut Option<StreamCollector>,
) {
    let deadline = Instant::now() + OUTPUT_DRAIN_GRACE;

    while !collectors_finished(stdout, stderr) && Instant::now() < deadline {
        let stdout_received = stdout.as_mut().is_some_and(StreamCollector::drain);
        let stderr_received = stderr.as_mut().is_some_and(StreamCollector::drain);
        if !stdout_received && !stderr_received {
            thread::sleep(OUTPUT_DRAIN_POLL_INTERVAL);
        }
    }
}

fn collectors_finished(stdout: &Option<StreamCollector>, stderr: &Option<StreamCollector>) -> bool {
    stdout.as_ref().is_none_or(|stream| stream.finished)
        && stderr.as_ref().is_none_or(|stream| stream.finished)
}

fn take_captured_stream(collector: &mut Option<StreamCollector>) -> Vec<u8> {
    collector
        .take()
        .map(|collector| collector.captured.into_bytes())
        .unwrap_or_default()
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

fn truncated_output_notice() -> Vec<u8> {
    format!("\n[chunk: output truncated after {MAX_CAPTURED_OUTPUT_BYTES} bytes]\n").into_bytes()
}

fn cancellation_requested(cancel: &Receiver<()>) -> bool {
    match cancel.try_recv() {
        Ok(()) | Err(TryRecvError::Disconnected) => true,
        Err(TryRecvError::Empty) => false,
    }
}

fn terminate_child(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    terminate_process_group_gracefully(child);
    if let Some(status) = wait_for_child_exit(child, TERMINATION_GRACE)? {
        return Ok(Some(status));
    }

    terminate_process_group_forcefully(child);
    wait_for_child_exit(child, TERMINATION_GRACE)
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(OUTPUT_DRAIN_POLL_INTERVAL);
    }
}

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

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let shell = env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "sh".to_string());
    let mut process = Command::new(shell);
    process.args(["-lc", command]);
    process
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("cmd");
    process.args(["/C", command]);
    process
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn parses_single_character_keys() {
        assert_eq!(CommandKey::parse("C").unwrap().display(), "C");
    }

    #[test]
    fn rejects_multi_character_keys() {
        assert!(CommandKey::parse("Ctrl-C").is_err());
    }

    #[test]
    fn rejects_empty_and_control_keys() {
        assert!(
            CommandKey::parse("")
                .unwrap_err()
                .to_string()
                .contains("cannot be empty")
        );
        assert!(
            CommandKey::parse("\u{7f}")
                .unwrap_err()
                .to_string()
                .contains("control character")
        );
    }

    #[test]
    fn exposes_raw_key_char_for_conflict_checks() {
        // Built-in conflict checks now consult the configured KeybindMap; see
        // `config::tests` and `keybind::tests`. CommandKey only reports its char.
        assert_eq!(CommandKey::parse("d").unwrap().char(), 'd');
        assert_eq!(CommandKey::parse("C").unwrap().char(), 'C');
    }

    #[test]
    fn key_matching_rejects_control_alt_and_different_keys() {
        let key = CommandKey::parse("C").unwrap();

        assert!(key.matches(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::NONE)));
        assert!(key.matches(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT)));
        assert!(!key.matches(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL)));
        assert!(!key.matches(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::ALT)));
        assert!(!key.matches(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
    }

    #[cfg(unix)]
    #[test]
    fn command_result_preserves_output_and_status() {
        let binding = binding();
        let cwd = PathBuf::from("/repo");

        let success =
            CustomCommandResult::from_output(&binding, cwd.clone(), output_status(0, "done\n", ""));
        assert_eq!(success.label(), "commit");
        assert_eq!(success.command(), "true");
        assert_eq!(success.cwd(), Some(Path::new("/repo")));
        assert_eq!(success.stdout(), "done\n");
        assert_eq!(success.stderr(), "");
        assert!(success.success());
        assert_eq!(success.status_text(), "exit 0");

        let failure =
            CustomCommandResult::from_output(&binding, cwd, output_status(7, "", "nope\n"));
        assert!(!failure.success());
        assert_eq!(failure.stderr(), "nope\n");
        assert_eq!(failure.status_text(), "exit 7");
    }

    #[test]
    fn not_started_result_reports_start_error() {
        let binding = binding();
        let result = CustomCommandResult::not_started(&binding, None, "missing shell");

        assert!(!result.success());
        assert_eq!(result.label(), "commit");
        assert_eq!(result.command(), "true");
        assert_eq!(result.cwd(), None);
        assert_eq!(result.stdout(), "");
        assert_eq!(result.stderr(), "missing shell");
        assert_eq!(result.status_text(), "failed to start: missing shell");
    }

    #[cfg(unix)]
    #[test]
    fn running_command_can_be_cancelled() {
        let binding = CustomCommandBinding::new(
            CommandKey::parse("C").unwrap(),
            "sleep".to_string(),
            "printf start; sleep 5; printf done".to_string(),
        );
        let (cancel_sender, cancel) = mpsc::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel_sender.send(()).unwrap();
        });

        let started_at = Instant::now();
        let result = run(&binding, PathBuf::from("."), cancel).unwrap();

        assert_eq!(result.status_text(), "cancelled");
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(result.stdout().contains("start"));
        assert!(!result.stdout().contains("done"));
    }

    #[cfg(unix)]
    #[test]
    fn command_output_is_bounded() {
        let binding = CustomCommandBinding::new(
            CommandKey::parse("C").unwrap(),
            "large output".to_string(),
            format!("yes chunk | head -c {}", MAX_CAPTURED_OUTPUT_BYTES + 1024),
        );
        let (_cancel_sender, cancel) = mpsc::channel();

        let result = run(&binding, PathBuf::from("."), cancel).unwrap();

        assert!(result.stdout().contains("output truncated"));
        assert!(
            result.stdout().len() <= MAX_CAPTURED_OUTPUT_BYTES + TRUNCATED_OUTPUT_NOTICE_MAX_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn command_returns_when_background_child_keeps_stdout_open() {
        let binding = CustomCommandBinding::new(
            CommandKey::parse("C").unwrap(),
            "background child".to_string(),
            "printf done; sleep 5 &".to_string(),
        );
        let (_cancel_sender, cancel) = mpsc::channel();

        let started_at = Instant::now();
        let result = run(&binding, PathBuf::from("."), cancel).unwrap();

        assert_eq!(result.status_text(), "exit 0");
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(result.stdout().contains("done"));
    }

    fn binding() -> CustomCommandBinding {
        CustomCommandBinding::new(
            CommandKey::parse("C").unwrap(),
            "commit".to_string(),
            "true".to_string(),
        )
    }

    #[cfg(unix)]
    fn output_status(code: i32, stdout: &str, stderr: &str) -> Output {
        use std::os::unix::process::ExitStatusExt;

        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }
}
