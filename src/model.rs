use std::sync::Arc;

/// A complete diff review session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    /// Human-readable title shown in the diff pane.
    pub title: String,
    /// Short description of the Git command or source refs behind the diff.
    pub source_label: String,
    /// Where this diff came from, used to decide whether actions like staging
    /// are available.
    pub source: DiffSource,
    /// Files in display order.
    pub files: Vec<DiffFile>,
}

/// Origin of a changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    /// Current worktree against `HEAD`.
    Worktree,
    /// Fixed Git references, usually merge-base to `HEAD` for PR review.
    GitRefs { old_ref: String, new_ref: String },
}

impl DiffSource {
    pub fn can_stage(&self) -> bool {
        matches!(self, Self::Worktree)
    }
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
