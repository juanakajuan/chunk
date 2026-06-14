//! User-configured shell command bindings and execution.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use color_eyre::eyre::{Result, eyre};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const RESERVED_KEYS: [char; 15] = [
    'q', '?', 'f', '/', 'j', 'k', 'n', 'N', 'g', 'G', ' ', 'd', 'e', 'a', 'y',
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
    stdout: String,
    stderr: String,
    status: CommandStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandStatus {
    success: bool,
    code: Option<i32>,
    start_error: Option<String>,
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
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: CommandStatus {
                success: output.status.success(),
                code: output.status.code(),
                start_error: None,
            },
        }
    }

    pub(crate) fn not_started(
        binding: &CustomCommandBinding,
        cwd: Option<PathBuf>,
        error: impl Into<String>,
    ) -> Self {
        let error = error.into();
        Self {
            label: binding.label.clone(),
            command: binding.command.clone(),
            cwd,
            stdout: String::new(),
            stderr: error.clone(),
            status: CommandStatus {
                success: false,
                code: None,
                start_error: Some(error),
            },
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
        &self.stdout
    }

    pub(crate) fn stderr(&self) -> &str {
        &self.stderr
    }

    pub(crate) fn success(&self) -> bool {
        self.status.success
    }

    pub(crate) fn status_text(&self) -> String {
        if let Some(error) = &self.status.start_error {
            return format!("failed to start: {error}");
        }

        match self.status.code {
            Some(code) => format!("exit {code}"),
            None => "terminated by signal".to_string(),
        }
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
    fn detects_builtin_key_conflicts() {
        assert!(CommandKey::parse("d").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("e").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("a").unwrap().conflicts_with_builtin());
        assert!(CommandKey::parse("y").unwrap().conflicts_with_builtin());
        assert!(!CommandKey::parse("C").unwrap().conflicts_with_builtin());
    }
}
