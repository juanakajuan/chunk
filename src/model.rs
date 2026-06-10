use std::sync::Arc;

/// A complete diff review session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    /// Human-readable title shown in the diff pane.
    pub title: String,
    /// Short description of the Git command or source refs behind the diff.
    pub source_label: String,
    /// Files in display order.
    pub files: Vec<DiffFile>,
}

/// One file entry in a parsed diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    /// Stable parser-assigned id used by the render cache.
    pub id: String,
    /// Path on the old side of the diff. Empty for additions when unavailable.
    pub old_path: String,
    /// Path on the new side of the diff. Empty for deletions when unavailable.
    pub path: String,
    /// Lazily loaded source prefix for the old side.
    pub old_source: SourceSnapshot,
    /// Lazily loaded source prefix for the new side.
    pub new_source: SourceSnapshot,
    pub status: FileStatus,
    pub stage: FileStage,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
}

impl DiffFile {
    /// User-facing path, falling back to the old path for deleted files.
    pub fn display_path(&self) -> &str {
        if self.path.is_empty() {
            &self.old_path
        } else {
            &self.path
        }
    }

    /// Number of unwrapped rows this diff would occupy.
    pub fn line_count(&self) -> usize {
        let file_header_rows = 1;

        if self.binary || self.hunks.is_empty() {
            return file_header_rows + 1;
        }

        file_header_rows
            + self
                .hunks
                .iter()
                .map(|hunk| hunk.lines.len() + 1)
                .sum::<usize>()
    }

    /// Best-effort line in the new file to place an external editor near.
    pub fn first_changed_line(&self) -> Option<u32> {
        if self.binary {
            return None;
        }

        self.hunks.iter().find_map(first_changed_line_in_hunk)
    }
}

impl FileStage {
    pub fn from_staged_unstaged(staged: bool, unstaged: bool) -> Self {
        match (staged, unstaged) {
            (true, true) => Self::Mixed,
            (true, false) => Self::Staged,
            (false, _) => Self::Unstaged,
        }
    }
}

fn first_changed_line_in_hunk(hunk: &DiffHunk) -> Option<u32> {
    let mut next_new_line = hunk.new_start.max(1);

    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::Added => return Some(valid_line_number(line.new_line, next_new_line)),
            DiffLineKind::Removed => return Some(next_new_line),
            DiffLineKind::Context => {
                if let Some(line_number) = line.new_line {
                    next_new_line = line_number.saturating_add(1).max(1);
                }
            }
            DiffLineKind::Meta => {}
        }
    }

    None
}

fn valid_line_number(line: Option<u32>, fallback: u32) -> u32 {
    line.filter(|line| *line > 0).unwrap_or(fallback)
}

/// Source text needed to seed syntax highlighting before a hunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SourceSnapshot {
    /// Not requested yet.
    #[default]
    Unloaded,
    /// Requested, but Git or the filesystem could not provide it.
    Unavailable,
    /// Loaded source prefix.
    Loaded(Arc<str>),
}

impl SourceSnapshot {
    pub fn loaded(source: String) -> Self {
        Self::Loaded(Arc::from(source))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Loaded(source) => Some(source.as_ref()),
            Self::Unloaded | Self::Unavailable => None,
        }
    }

    pub fn is_unloaded(&self) -> bool {
        matches!(self, Self::Unloaded)
    }
}

/// Git file status shown in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

impl FileStatus {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Modified => "M",
            Self::Renamed => "R",
            Self::Copied => "C",
        }
    }
}

/// Staging state for a worktree file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStage {
    Unstaged,
    Staged,
    Mixed,
}

/// A unified diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub stage: FileStage,
    pub lines: Vec<DiffLine>,
}

/// A single parsed line inside a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub content: String,
}

/// Rendering and line-number semantics for a parsed hunk line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Meta,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_changed_line_skips_context_prefix() {
        let file = text_file(
            10,
            vec![
                diff_line(DiffLineKind::Context, Some(10), Some(10)),
                diff_line(DiffLineKind::Added, None, Some(11)),
            ],
        );

        assert_eq!(file.first_changed_line(), Some(11));
    }

    #[test]
    fn first_changed_line_for_removal_uses_current_new_line() {
        let file = text_file(
            20,
            vec![
                diff_line(DiffLineKind::Context, Some(20), Some(20)),
                diff_line(DiffLineKind::Removed, Some(21), None),
            ],
        );

        assert_eq!(file.first_changed_line(), Some(21));
    }

    #[test]
    fn first_changed_line_ignores_binary_files() {
        let mut file = text_file(1, vec![diff_line(DiffLineKind::Added, None, Some(1))]);
        file.binary = true;

        assert_eq!(file.first_changed_line(), None);
    }

    fn text_file(new_start: u32, lines: Vec<DiffLine>) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: "sample.txt".to_string(),
            path: "sample.txt".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: format!("@@ -{new_start} +{new_start} @@"),
                old_start: new_start,
                old_lines: 1,
                new_start,
                new_lines: 1,
                stage: FileStage::Unstaged,
                lines,
            }],
            binary: false,
        }
    }

    fn diff_line(kind: DiffLineKind, old_line: Option<u32>, new_line: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_line,
            new_line,
            content: "line".to_string(),
        }
    }
}
