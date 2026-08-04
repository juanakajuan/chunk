//! File and sidebar identity rules for replacing a review session.
//!
//! Diff-local reload position belongs to `SelectedDiffPane`.

use crate::model::{Changeset, DiffFile};
use crate::rows::SidebarRowTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewSessionSnapshot {
    selected_file_identity: Option<String>,
    selected_file_index: usize,
    sidebar_target: Option<SidebarRowTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewSessionReplacement {
    pub(super) changeset: Changeset,
    pub(super) selected_file_index: usize,
    pub(super) kept_file_selection: bool,
    pub(super) sidebar_target: Option<SidebarRowTarget>,
}

impl ReviewSessionSnapshot {
    pub(super) fn capture(
        selected_file: Option<&DiffFile>,
        selected_file_index: usize,
        sidebar_target: Option<SidebarRowTarget>,
    ) -> Self {
        Self {
            selected_file_identity: selected_file.map(file_identity),
            selected_file_index,
            sidebar_target,
        }
    }
}

impl ReviewSessionReplacement {
    pub(super) fn plan(snapshot: ReviewSessionSnapshot, changeset: Changeset) -> Self {
        let reselected_file_index = snapshot
            .selected_file_identity
            .as_deref()
            .and_then(|identity| find_file_index(&changeset, identity));
        let fallback_index = snapshot
            .selected_file_index
            .min(changeset.files.len().saturating_sub(1));
        let kept_file_selection = reselected_file_index.is_some();
        let selected_file_index = reselected_file_index.unwrap_or(fallback_index);
        let sidebar_target = reloaded_sidebar_target(snapshot.sidebar_target, &changeset);

        Self {
            changeset,
            selected_file_index,
            kept_file_selection,
            sidebar_target,
        }
    }
}

fn file_identity(file: &DiffFile) -> String {
    file.display_path().to_string()
}

fn find_file_index(changeset: &Changeset, identity: &str) -> Option<usize> {
    changeset
        .files
        .iter()
        .position(|file| file.display_path() == identity)
}

fn reloaded_sidebar_target(
    previous_target: Option<SidebarRowTarget>,
    changeset: &Changeset,
) -> Option<SidebarRowTarget> {
    match previous_target {
        Some(SidebarRowTarget::Folder(path)) if folder_exists(changeset, &path) => {
            Some(SidebarRowTarget::Folder(path))
        }
        _ => None,
    }
}

fn folder_exists(changeset: &Changeset, folder_path: &str) -> bool {
    if folder_path.is_empty() {
        return changeset
            .files
            .iter()
            .any(|file| !file.display_path().contains('/'));
    }

    changeset
        .files
        .iter()
        .any(|file| path_is_inside_folder(file.display_path(), folder_path))
}

fn path_is_inside_folder(path: &str, folder_path: &str) -> bool {
    path.strip_prefix(folder_path)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiffHunk, DiffLine, DiffLineKind, FileStage, FileStatus, SourceSnapshot};

    #[test]
    fn replacement_preserves_file_and_folder_by_identity() {
        let current = changeset(vec![
            file("src/a.rs", vec![hunk(1, 1, 1, 1)]),
            file("src/b.rs", vec![hunk(4, 1, 8, 1), hunk(20, 2, 30, 2)]),
        ]);
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[1]),
            1,
            Some(SidebarRowTarget::Folder("src".to_string())),
        );

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![
                file("src/b.rs", vec![hunk(4, 1, 8, 1), hunk(20, 2, 30, 2)]),
                file("src/a.rs", vec![hunk(1, 1, 1, 1)]),
            ]),
        );

        assert_eq!(replacement.selected_file_index, 0);
        assert!(replacement.kept_file_selection);
        assert_eq!(
            replacement.sidebar_target,
            Some(SidebarRowTarget::Folder("src".to_string()))
        );
    }

    #[test]
    fn replacement_resets_position_when_selected_file_disappears() {
        let current = changeset(vec![
            file("a.rs", vec![hunk(1, 1, 1, 1)]),
            file("b.rs", vec![hunk(5, 1, 5, 1)]),
        ]);
        let snapshot = ReviewSessionSnapshot::capture(Some(&current.files[1]), 1, None);

        let replacement =
            ReviewSessionReplacement::plan(snapshot, changeset(vec![file("a.rs", vec![])]));

        assert_eq!(replacement.selected_file_index, 0);
        assert!(!replacement.kept_file_selection);
    }

    #[test]
    fn replacement_reports_when_selected_file_is_kept() {
        let current = changeset(vec![file(
            "sample.rs",
            vec![hunk(1, 1, 1, 1), hunk(10, 1, 10, 1), hunk(20, 1, 20, 1)],
        )]);
        let snapshot = ReviewSessionSnapshot::capture(Some(&current.files[0]), 0, None);

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![file("sample.rs", vec![hunk(1, 1, 1, 1)])]),
        );

        assert_eq!(replacement.selected_file_index, 0);
        assert!(replacement.kept_file_selection);
    }

    #[test]
    fn replacement_drops_folder_target_when_folder_disappears() {
        let current = changeset(vec![file("src/a.rs", vec![hunk(1, 1, 1, 1)])]);
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[0]),
            0,
            Some(SidebarRowTarget::Folder("src".to_string())),
        );

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![file("tests/a.rs", vec![hunk(1, 1, 1, 1)])]),
        );

        assert_eq!(replacement.sidebar_target, None);
    }

    #[test]
    fn replacement_drops_root_target_when_root_file_disappears() {
        let current = changeset(vec![file("README.md", vec![hunk(1, 1, 1, 1)])]);
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[0]),
            0,
            Some(SidebarRowTarget::Folder(String::new())),
        );

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![file("src/lib.rs", vec![hunk(1, 1, 1, 1)])]),
        );

        assert_eq!(replacement.sidebar_target, None);
    }

    fn changeset(files: Vec<DiffFile>) -> Changeset {
        Changeset {
            title: "Test".to_string(),
            source_label: "test".to_string(),
            files,
        }
    }

    fn file(path: &str, hunks: Vec<DiffHunk>) -> DiffFile {
        DiffFile {
            id: path.to_string(),
            old_path: path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::default(),
            new_source: SourceSnapshot::default(),
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks,
            binary: false,
        }
    }

    fn hunk(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> DiffHunk {
        DiffHunk {
            header: format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@"),
            old_start,
            old_lines,
            new_start,
            new_lines,
            stage: FileStage::Unstaged,
            lines: vec![DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(old_start),
                new_line: Some(new_start),
                content: "line".to_string(),
            }],
        }
    }
}
