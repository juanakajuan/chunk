//! The outcome of running an external process in the background.
//!
//! `ask_ai` and `custom_command` both spawn a child process and report the same
//! shape of result: captured stdout/stderr plus a status that is either a
//! completed exit code, a cancellation, or a failure to start. This module owns
//! that shape and its status-text formatting once. Each caller wraps a
//! `ProcessOutcome` alongside its own request metadata, so the difference
//! between the two is only what each remembers about the request — never how a
//! process result is built or described.

use std::process::Output;

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
