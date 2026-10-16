//! JSON contract for external review frontends.

use std::io::{self, Write};

use color_eyre::eyre::Result;
use serde::Serialize;

use crate::model::{Changeset, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStage, FileStatus};

pub(crate) fn print(changeset: &Changeset) -> Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &JsonChangeset::from(changeset))?;
    writeln!(handle)?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct JsonChangeset<'a> {
    title: &'a str,
    source_label: &'a str,
    files: Vec<JsonFile<'a>>,
}

#[derive(Debug, Serialize)]
struct JsonFile<'a> {
    path: &'a str,
    old_path: &'a str,
    status: &'static str,
    stage: &'static str,
    additions: usize,
    deletions: usize,
    binary: bool,
    hunks: Vec<JsonHunk<'a>>,
}

#[derive(Debug, Serialize)]
struct JsonHunk<'a> {
    header: &'a str,
    old_range: JsonLineRange,
    new_range: JsonLineRange,
    stage: &'static str,
    lines: Vec<JsonLine<'a>>,
}

#[derive(Debug, Serialize)]
struct JsonLineRange {
    start: u32,
    lines: u32,
}

#[derive(Debug, Serialize)]
struct JsonLine<'a> {
    kind: &'static str,
    old_line: Option<u32>,
    new_line: Option<u32>,
    content: &'a str,
}

impl<'a> From<&'a Changeset> for JsonChangeset<'a> {
    fn from(changeset: &'a Changeset) -> Self {
        Self {
            title: &changeset.title,
            source_label: &changeset.source_label,
            files: changeset.files.iter().map(JsonFile::from).collect(),
        }
    }
}

impl<'a> From<&'a DiffFile> for JsonFile<'a> {
    fn from(file: &'a DiffFile) -> Self {
        Self {
            path: &file.path,
            old_path: &file.old_path,
            status: file_status(file.status),
            stage: file_stage(file.stage),
            additions: file.additions,
            deletions: file.deletions,
            binary: file.binary,
            hunks: file.hunks.iter().map(JsonHunk::from).collect(),
        }
    }
}

impl<'a> From<&'a DiffHunk> for JsonHunk<'a> {
    fn from(hunk: &'a DiffHunk) -> Self {
        Self {
            header: &hunk.header,
            old_range: JsonLineRange {
                start: hunk.old_start,
                lines: hunk.old_lines,
            },
            new_range: JsonLineRange {
                start: hunk.new_start,
                lines: hunk.new_lines,
            },
            stage: file_stage(hunk.stage),
            lines: hunk.lines.iter().map(JsonLine::from).collect(),
        }
    }
}

impl<'a> From<&'a DiffLine> for JsonLine<'a> {
    fn from(line: &'a DiffLine) -> Self {
        Self {
            kind: line_kind(line.kind),
            old_line: line.old_line,
            new_line: line.new_line,
            content: &line.content,
        }
    }
}

fn file_status(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "added",
        FileStatus::Deleted => "deleted",
        FileStatus::Modified => "modified",
        FileStatus::Renamed => "renamed",
        FileStatus::Copied => "copied",
    }
}

fn file_stage(stage: FileStage) -> &'static str {
    match stage {
        FileStage::Unstaged => "unstaged",
        FileStage::Staged => "staged",
        FileStage::Mixed => "mixed",
        FileStage::Committed => "committed",
    }
}

fn line_kind(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Context => "context",
        DiffLineKind::Added => "added",
        DiffLineKind::Removed => "removed",
        DiffLineKind::Meta => "meta",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::{DiffHunk, DiffLine, SourceSnapshot};

    #[test]
    fn serializes_documented_diff_contract() {
        let changeset = Changeset {
            title: "Working tree changes (main)".to_string(),
            source_label: "git diff HEAD + untracked".to_string(),
            files: vec![DiffFile {
                id: "0".to_string(),
                old_path: "src/old.rs".to_string(),
                path: "src/new.rs".to_string(),
                old_source: SourceSnapshot::Unloaded,
                new_source: SourceSnapshot::Unloaded,
                status: FileStatus::Renamed,
                stage: FileStage::Mixed,
                additions: 1,
                deletions: 1,
                binary: false,
                hunks: vec![DiffHunk {
                    header: "@@ -10,2 +10,2 @@".to_string(),
                    old_start: 10,
                    old_lines: 2,
                    new_start: 10,
                    new_lines: 2,
                    stage: FileStage::Staged,
                    lines: vec![
                        DiffLine {
                            kind: DiffLineKind::Removed,
                            old_line: Some(10),
                            new_line: None,
                            content: "old line".to_string(),
                        },
                        DiffLine {
                            kind: DiffLineKind::Added,
                            old_line: None,
                            new_line: Some(10),
                            content: "new line".to_string(),
                        },
                    ],
                }],
            }],
        };

        let value = serde_json::to_value(JsonChangeset::from(&changeset)).unwrap();

        assert_eq!(
            value,
            json!({
                "title": "Working tree changes (main)",
                "source_label": "git diff HEAD + untracked",
                "files": [{
                    "path": "src/new.rs",
                    "old_path": "src/old.rs",
                    "status": "renamed",
                    "stage": "mixed",
                    "additions": 1,
                    "deletions": 1,
                    "binary": false,
                    "hunks": [{
                        "header": "@@ -10,2 +10,2 @@",
                        "old_range": { "start": 10, "lines": 2 },
                        "new_range": { "start": 10, "lines": 2 },
                        "stage": "staged",
                        "lines": [{
                            "kind": "removed",
                            "old_line": 10,
                            "new_line": null,
                            "content": "old line"
                        }, {
                            "kind": "added",
                            "old_line": null,
                            "new_line": 10,
                            "content": "new line"
                        }]
                    }]
                }]
            })
        );
    }
}
