//! Read-only OpenCode requests for asking questions about the current diff.
//!
//! This module owns prompt/context construction and the OpenCode process
//! boundary. The app owns UI state; runtime owns task orchestration.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::model::{DiffFile, DiffHunk, DiffLine, DiffLineKind};

const OPENCODE_PROGRAM: &str = "opencode";
const REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_DIFF_CONTEXT_CHARS: usize = 12_000;
const READ_ONLY_CONFIG_CONTENT: &str = r#"{"$schema":"https://opencode.ai/config.json","autoupdate":false,"share":"disabled","permission":{"*":"deny","read":{"*":"allow","*.env":"deny","*.env.*":"deny","*.env.example":"allow"},"glob":"allow","grep":"allow","lsp":"allow","edit":"deny","bash":"deny","task":"deny","skill":"deny","webfetch":"allow","websearch":"allow","external_directory":"deny"}}"#;
const EXPLAIN_CODE_PROMPT: &str = "Explain the selected or focused code for a code review. Describe what the code does, why the changed code matters in this review context, and any assumptions or risks that affect review. Inspect surrounding repository context read-only if needed.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskAiRequest {
    question: String,
    context: AskAiContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskAiContext {
    review_mode: AskAiReviewMode,
    changeset_title: String,
    source_label: String,
    file_path: String,
    old_path: String,
    selected_text: Option<String>,
    focused_hunk: Option<AskAiHunkContext>,
    diff_text: String,
    binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskAiReviewMode {
    Worktree,
    PullRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AskAiHunkContext {
    index: usize,
    header: String,
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskAiResult {
    request: AskAiRequest,
    repo_root: Option<PathBuf>,
    stdout: String,
    stderr: String,
    status: AskAiStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AskAiStatus {
    success: bool,
    code: Option<i32>,
    cancelled: bool,
    start_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskAiInvocation {
    program: String,
    args: Vec<String>,
    current_dir: PathBuf,
    env: Vec<(String, String)>,
}

impl AskAiRequest {
    pub(crate) fn new(question: String, context: AskAiContext) -> Self {
        Self { question, context }
    }

    pub(crate) fn explain_code(context: AskAiContext) -> Self {
        Self {
            question: EXPLAIN_CODE_PROMPT.to_string(),
            context,
        }
    }

    pub(crate) fn question(&self) -> &str {
        &self.question
    }

    pub(crate) fn context(&self) -> &AskAiContext {
        &self.context
    }
}

impl AskAiContext {
    pub(crate) fn focused(
        review_mode: AskAiReviewMode,
        changeset_title: String,
        source_label: String,
        file: &DiffFile,
        hunk_index: Option<usize>,
        selected_text: Option<String>,
    ) -> Self {
        let focused_hunk = hunk_index
            .and_then(|index| file.hunks.get(index).map(|hunk| (index, hunk)))
            .map(|(index, hunk)| AskAiHunkContext::new(index, hunk));
        let diff_text = focused_hunk
            .as_ref()
            .and_then(|hunk| file.hunks.get(hunk.index))
            .map_or_else(|| file_diff_text(file), |hunk| hunk_diff_text(file, hunk));

        Self {
            review_mode,
            changeset_title,
            source_label,
            file_path: file.display_path().to_string(),
            old_path: file.old_path.clone(),
            selected_text: selected_text.and_then(non_empty_text),
            focused_hunk,
            diff_text: truncate_context(diff_text),
            binary: file.binary,
        }
    }

    pub(crate) fn summary(&self) -> String {
        match &self.focused_hunk {
            Some(hunk) => format!("{} hunk {}", self.file_path, hunk.index + 1),
            None => self.file_path.clone(),
        }
    }
}

impl AskAiReviewMode {
    fn label(self) -> &'static str {
        match self {
            Self::Worktree => "worktree diff",
            Self::PullRequest => "pull request review",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::Worktree => {
                "Worktree mode may refer to editable files, but this request is read-only."
            }
            Self::PullRequest => {
                "PR mode reviews a ref diff; do not assume files are editable snapshots."
            }
        }
    }
}

impl AskAiHunkContext {
    fn new(index: usize, hunk: &DiffHunk) -> Self {
        Self {
            index,
            header: hunk.header.clone(),
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
        }
    }

    fn prompt_lines(&self) -> String {
        format!(
            "Focused hunk: #{} {}\nOld range: start {}, lines {}\nNew range: start {}, lines {}",
            self.index + 1,
            self.header,
            self.old_start,
            self.old_lines,
            self.new_start,
            self.new_lines
        )
    }
}

impl AskAiResult {
    pub(crate) fn from_output(request: AskAiRequest, repo_root: PathBuf, output: Output) -> Self {
        Self {
            request,
            repo_root: Some(repo_root),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: AskAiStatus {
                success: output.status.success(),
                code: output.status.code(),
                cancelled: false,
                start_error: None,
            },
        }
    }

    pub(crate) fn cancelled(
        request: AskAiRequest,
        repo_root: PathBuf,
        output: Option<Output>,
    ) -> Self {
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
            request,
            repo_root: Some(repo_root),
            stdout,
            stderr,
            status: AskAiStatus {
                success: false,
                code,
                cancelled: true,
                start_error: None,
            },
        }
    }

    pub(crate) fn not_started(
        request: AskAiRequest,
        repo_root: Option<PathBuf>,
        error: impl Into<String>,
    ) -> Self {
        let error = error.into();
        Self {
            request,
            repo_root,
            stdout: String::new(),
            stderr: error.clone(),
            status: AskAiStatus {
                success: false,
                code: None,
                cancelled: false,
                start_error: Some(error),
            },
        }
    }

    pub(crate) fn question(&self) -> &str {
        self.request.question()
    }

    pub(crate) fn context_summary(&self) -> String {
        self.request.context().summary()
    }

    pub(crate) fn repo_root(&self) -> Option<&Path> {
        self.repo_root.as_deref()
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

impl AskAiInvocation {
    pub(crate) fn new(request: &AskAiRequest, repo_root: &Path) -> Self {
        Self {
            program: OPENCODE_PROGRAM.to_string(),
            args: vec![
                "run".to_string(),
                "--pure".to_string(),
                "--format".to_string(),
                "default".to_string(),
                "--dir".to_string(),
                repo_root.display().to_string(),
                "--title".to_string(),
                session_title(request),
                build_prompt(request, repo_root),
            ],
            current_dir: repo_root.to_path_buf(),
            env: vec![
                (
                    "OPENCODE_CONFIG_CONTENT".to_string(),
                    READ_ONLY_CONFIG_CONTENT.to_string(),
                ),
                ("NO_COLOR".to_string(), "1".to_string()),
            ],
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.current_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}

pub(crate) fn run(
    request: AskAiRequest,
    repo_root: PathBuf,
    cancel: Receiver<()>,
) -> io::Result<AskAiResult> {
    let invocation = AskAiInvocation::new(&request, &repo_root);
    let mut child = invocation.command().spawn()?;

    loop {
        if cancellation_requested(&cancel) {
            let _ = child.kill();
            let output = child.wait_with_output().ok();
            return Ok(AskAiResult::cancelled(request, repo_root, output));
        }

        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            return Ok(AskAiResult::from_output(request, repo_root, output));
        }

        thread::sleep(REQUEST_POLL_INTERVAL);
    }
}

fn build_prompt(request: &AskAiRequest, repo_root: &Path) -> String {
    let context = request.context();
    let mut prompt = String::new();

    prompt.push_str("You are answering a question from chunk, a terminal diff reviewer.\n");
    prompt.push_str("Read-only enforcement:\n");
    prompt.push_str("- Do not edit, write, patch, stage, commit, push, delete, install, or run mutating commands.\n");
    prompt.push_str(
        "- If more context is needed, inspect the repository with read-only tools only.\n",
    );
    prompt.push_str(
        "- Internet lookup is allowed with read-only web fetch/search tools when useful.\n",
    );
    prompt.push_str(
        "- The OpenCode process is launched with edit, bash, task, skill, and external-directory permissions denied.\n\n",
    );
    prompt.push_str("User question:\n");
    prompt.push_str(request.question());
    prompt.push_str("\n\nStructured review context:\n");
    prompt.push_str(&format!("Repository root: {}\n", repo_root.display()));
    prompt.push_str(&format!("Review mode: {}\n", context.review_mode.label()));
    prompt.push_str(context.review_mode.note());
    prompt.push('\n');
    prompt.push_str(&format!("Changeset title: {}\n", context.changeset_title));
    prompt.push_str(&format!("Review source: {}\n", context.source_label));
    prompt.push_str(&format!("Focused file: {}\n", context.file_path));
    if !context.old_path.is_empty() && context.old_path != context.file_path {
        prompt.push_str(&format!("Old file path: {}\n", context.old_path));
    }
    if context.binary {
        prompt.push_str("Focused file is binary.\n");
    }
    if let Some(hunk) = &context.focused_hunk {
        prompt.push_str(&hunk.prompt_lines());
        prompt.push('\n');
    }
    if let Some(selection) = &context.selected_text {
        prompt.push_str("\nSelected visible text:\n");
        prompt.push_str(selection);
        prompt.push('\n');
    }
    prompt.push_str("\nDiff context with old/new line columns:\n```diff\n");
    prompt.push_str(&context.diff_text);
    if !context.diff_text.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push_str("```\n");

    prompt
}

fn session_title(request: &AskAiRequest) -> String {
    const MAX_TITLE_CHARS: usize = 80;

    let title = format!(
        "chunk Ask AI: {}: {}",
        request.context().summary(),
        request.question()
    );
    truncate_chars(&title, MAX_TITLE_CHARS)
}

fn file_diff_text(file: &DiffFile) -> String {
    let mut text = diff_header(file);
    if file.binary {
        text.push_str("Binary file changed\n");
        return text;
    }
    if file.hunks.is_empty() {
        text.push_str("File changed without textual hunks\n");
        return text;
    }

    for hunk in &file.hunks {
        push_hunk_text(&mut text, hunk);
    }
    text
}

fn hunk_diff_text(file: &DiffFile, hunk: &DiffHunk) -> String {
    let mut text = diff_header(file);
    push_hunk_text(&mut text, hunk);
    text
}

fn diff_header(file: &DiffFile) -> String {
    let old_path = if file.old_path.is_empty() {
        file.display_path()
    } else {
        &file.old_path
    };
    let new_path = if file.path.is_empty() {
        file.display_path()
    } else {
        &file.path
    };

    format!("diff --git a/{old_path} b/{new_path}\n--- a/{old_path}\n+++ b/{new_path}\n")
}

fn push_hunk_text(text: &mut String, hunk: &DiffHunk) {
    text.push_str(&hunk.header);
    text.push('\n');
    for line in &hunk.lines {
        text.push_str(&format_diff_line(line));
        text.push('\n');
    }
}

fn format_diff_line(line: &DiffLine) -> String {
    format!(
        "{:>4} {:>4} {}{}",
        line_number(line.old_line),
        line_number(line.new_line),
        diff_marker(line.kind),
        line.content
    )
}

fn line_number(line: Option<u32>) -> String {
    line.map_or_else(|| "-".to_string(), |line| line.to_string())
}

fn diff_marker(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Context | DiffLineKind::Meta => " ",
        DiffLineKind::Added => "+",
        DiffLineKind::Removed => "-",
    }
}

fn truncate_context(text: String) -> String {
    if text.chars().count() <= MAX_DIFF_CONTEXT_CHARS {
        return text;
    }

    format!(
        "{}\n[diff context truncated]\n",
        truncate_chars(&text, MAX_DIFF_CONTEXT_CHARS)
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn non_empty_text(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

fn cancellation_requested(cancel: &Receiver<()>) -> bool {
    match cancel.try_recv() {
        Ok(()) | Err(TryRecvError::Disconnected) => true,
        Err(TryRecvError::Empty) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiffLineKind, FileStage, FileStatus, SourceSnapshot};

    #[test]
    fn prompt_includes_question_context_and_read_only_constraints() {
        let file = diff_file_with_hunk();
        let context = AskAiContext::focused(
            AskAiReviewMode::PullRequest,
            "PR review feature into main".to_string(),
            "git diff main...HEAD".to_string(),
            &file,
            Some(0),
            Some("selected code".to_string()),
        );
        let request = AskAiRequest::new("Why did this change?".to_string(), context);

        let prompt = build_prompt(&request, Path::new("/repo"));

        assert!(prompt.contains("Why did this change?"));
        assert!(prompt.contains("Repository root: /repo"));
        assert!(prompt.contains("Review mode: pull request review"));
        assert!(prompt.contains("Focused file: src/lib.rs"));
        assert!(prompt.contains("Focused hunk: #1 @@ -1,2 +1,2 @@"));
        assert!(prompt.contains("selected code"));
        assert!(prompt.contains("+new line"));
        assert!(prompt.contains("Do not edit, write, patch, stage, commit, push"));
        assert!(prompt.contains("Internet lookup is allowed"));
        assert!(
            prompt.contains("edit, bash, task, skill, and external-directory permissions denied")
        );
    }

    #[test]
    fn read_only_invocation_overrides_mutating_permissions() {
        let file = diff_file_with_hunk();
        let context = AskAiContext::focused(
            AskAiReviewMode::Worktree,
            "Tracked changes".to_string(),
            "git diff HEAD + untracked".to_string(),
            &file,
            Some(0),
            None,
        );
        let request = AskAiRequest::new("Explain this".to_string(), context);

        let invocation = AskAiInvocation::new(&request, Path::new("/repo"));

        assert_eq!(invocation.program, "opencode");
        assert!(invocation.args.contains(&"run".to_string()));
        assert!(invocation.args.contains(&"--pure".to_string()));
        assert!(
            invocation
                .args
                .windows(2)
                .any(|args| args == ["--dir", "/repo"])
        );
        assert!(
            !invocation
                .args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );

        let config = invocation
            .env
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
            .map(|(_, value)| value.as_str())
            .expect("inline config should be set");
        assert!(config.contains(r#""*":"deny""#));
        assert!(config.contains(r#""read":{"*":"allow""#));
        assert!(config.contains(r#""edit":"deny""#));
        assert!(config.contains(r#""bash":"deny""#));
        assert!(config.contains(r#""webfetch":"allow""#));
        assert!(config.contains(r#""websearch":"allow""#));
        assert!(config.contains(r#""external_directory":"deny""#));
        assert!(config.contains(r#""autoupdate":false"#));
    }

    #[test]
    fn explain_code_prompt_is_review_oriented_and_uses_context() {
        let file = diff_file_with_hunk();
        let context = AskAiContext::focused(
            AskAiReviewMode::Worktree,
            "Tracked changes".to_string(),
            "git diff HEAD + untracked".to_string(),
            &file,
            Some(0),
            Some("selected code".to_string()),
        );
        let request = AskAiRequest::explain_code(context);

        let prompt = build_prompt(&request, Path::new("/repo"));

        assert!(prompt.contains("Explain the selected or focused code"));
        assert!(prompt.contains("what the code does"));
        assert!(prompt.contains("why the changed code matters"));
        assert!(prompt.contains("assumptions or risks"));
        assert!(prompt.contains("Focused hunk: #1 @@ -1,2 +1,2 @@"));
        assert!(prompt.contains("selected code"));
        assert!(prompt.contains("+new line"));
        assert!(prompt.contains("read-only"));
    }

    fn diff_file_with_hunk() -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: "src/lib.rs".to_string(),
            path: "src/lib.rs".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 1,
            deletions: 1,
            hunks: vec![DiffHunk {
                header: "@@ -1,2 +1,2 @@".to_string(),
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                stage: FileStage::Unstaged,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Removed,
                        old_line: Some(1),
                        new_line: None,
                        content: "old line".to_string(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        old_line: None,
                        new_line: Some(1),
                        content: "new line".to_string(),
                    },
                ],
            }],
            binary: false,
        }
    }
}
