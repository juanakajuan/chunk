//! External editor command resolution.
//!
//! The app decides which worktree file should be opened. Runtime owns terminal
//! suspension, then uses this module to resolve `$EDITOR` and run it.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const NO_EDITOR_CONFIGURED: &str = "no editor configured; set $EDITOR to open files";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorRequest {
    pub(crate) path: PathBuf,
    pub(crate) line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorCommand {
    program: String,
    args: Vec<String>,
}

impl EditorCommand {
    pub(crate) fn from_env() -> Result<Self, String> {
        let raw = env::var("EDITOR").map_err(|_| NO_EDITOR_CONFIGURED.to_string())?;
        Self::parse(&raw)
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let mut parts = raw.split_whitespace();
        let Some(program) = parts.next() else {
            return Err(NO_EDITOR_CONFIGURED.to_string());
        };

        Ok(Self {
            program: program.to_string(),
            args: parts.map(str::to_string).collect(),
        })
    }

    pub(crate) fn status(&self, request: &EditorRequest) -> std::io::Result<ExitStatus> {
        let mut command = Command::new(&self.program);
        command.args(&self.args);

        if supports_plus_line(&self.program)
            && let Some(line) = request.line
        {
            command.arg(format!("+{line}"));
        }
        command.arg(&request.path);

        command.status()
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.program
    }
}

fn supports_plus_line(program: &str) -> bool {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);

    matches!(
        name,
        "emacs" | "emacsclient" | "gvim" | "nano" | "nvim" | "vi" | "view" | "vim"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_editor_program_and_arguments() {
        let editor = EditorCommand::parse("nvim --wait -p").unwrap();

        assert_eq!(editor.program, "nvim");
        assert_eq!(editor.args, ["--wait", "-p"]);
        assert_eq!(editor.display_name(), "nvim");
    }

    #[test]
    fn rejects_empty_editor_values() {
        assert_eq!(
            EditorCommand::parse(" \t ").unwrap_err(),
            NO_EDITOR_CONFIGURED
        );
    }

    #[test]
    fn plus_line_support_uses_program_basename() {
        assert!(supports_plus_line("/usr/bin/nvim"));
        assert!(supports_plus_line("vim"));
        assert!(supports_plus_line("emacsclient"));
        assert!(!supports_plus_line("code"));
    }
}
