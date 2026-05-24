#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    pub title: String,
    pub source_label: String,
    pub files: Vec<DiffFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub id: String,
    pub old_path: String,
    pub path: String,
    pub status: FileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
}

impl DiffFile {
    pub fn display_path(&self) -> &str {
        if self.path.is_empty() {
            &self.old_path
        } else {
            &self.path
        }
    }

    pub fn line_count(&self) -> usize {
        if self.binary || self.hunks.is_empty() {
            return 1;
        }

        self.hunks.iter().map(|hunk| hunk.lines.len() + 1).sum()
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Meta,
}
