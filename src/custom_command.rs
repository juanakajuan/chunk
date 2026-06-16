//! User-configured shell command bindings and execution.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use color_eyre::eyre::{Result, eyre};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::process::ProcessOutcome;

const RESERVED_KEYS: [char; 16] = [
    'q', '?', 'f', '/', 'j', 'k', 'n', 'N', 'g', 'G', ' ', 'd', 'e', 'a', 'x', 'y',
];

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

    pub(crate) fn conflicts_with_builtin(self) -> bool {
        RESERVED_KEYS.contains(&self.value)
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
) -> std::io::Result<CustomCommandResult> {
    shell_command(binding.command())
        .current_dir(&cwd)
        .output()
        .map(|output| CustomCommandResult::from_output(binding, cwd, output))
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
    fn detects_builtin_key_conflicts() {
        assert!(CommandKey::parse("d").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("e").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("a").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("x").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("y").unwrap().conflicts_with_builtin());
        assert!(!CommandKey::parse("C").unwrap().conflicts_with_builtin());
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
