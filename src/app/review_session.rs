//! Session replacement rules for preserving review position across reloads.

use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::rows::SidebarRowTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewSessionSnapshot {
    selected_file_identity: Option<String>,
    selected_file_index: usize,
    selected_hunk_identity: Option<HunkIdentity>,
    selected_hunk_index: Option<usize>,
    diff_scroll: usize,
    sidebar_target: Option<SidebarRowTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewSessionReplacement {
    pub(super) changeset: Changeset,
    pub(super) selected_file_index: usize,
    pub(super) selected_hunk_index: Option<usize>,
    pub(super) diff_scroll: usize,
    pub(super) sidebar_target: Option<SidebarRowTarget>,
}

impl ReviewSessionSnapshot {
    pub(super) fn capture(
        selected_file: Option<&DiffFile>,
        selected_file_index: usize,
        selected_hunk: Option<&DiffHunk>,
        selected_hunk_index: Option<usize>,
        diff_scroll: usize,
        sidebar_target: Option<SidebarRowTarget>,
    ) -> Self {
        Self {
            selected_file_identity: selected_file.map(file_identity),
            selected_file_index,
            selected_hunk_identity: selected_hunk.map(hunk_identity),
            selected_hunk_index,
            diff_scroll,
            sidebar_target,
        }
    }
}

impl ReviewSessionReplacement {
    pub(super) fn plan(
        snapshot: ReviewSessionSnapshot,
        changeset: Changeset,
        preserve_scroll: bool,
    ) -> Self {
        let reselected_file_index = snapshot
            .selected_file_identity
            .as_deref()
            .and_then(|identity| find_file_index(&changeset, identity));
        let fallback_index = snapshot
            .selected_file_index
            .min(changeset.files.len().saturating_sub(1));
        let kept_file_selection = reselected_file_index.is_some();
        let selected_file_index = reselected_file_index.unwrap_or(fallback_index);
        let selected_hunk_index = reloaded_hunk_index(
            changeset.files.get(selected_file_index),
            kept_file_selection,
            snapshot.selected_hunk_identity,
            snapshot.selected_hunk_index,
        );
        let diff_scroll = if preserve_scroll && kept_file_selection {
            snapshot.diff_scroll
        } else {
            0
        };
        let sidebar_target = reloaded_sidebar_target(snapshot.sidebar_target, &changeset);

        Self {
            changeset,
            selected_file_index,
            selected_hunk_index,
            diff_scroll,
            sidebar_target,
        }
    }
}

fn file_identity(file: &DiffFile) -> String {
    file.display_path().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HunkIdentity {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

fn hunk_identity(hunk: &DiffHunk) -> HunkIdentity {
    HunkIdentity {
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
    }
}

fn reloaded_hunk_index(
    file: Option<&DiffFile>,
    kept_file_selection: bool,
    previous_identity: Option<HunkIdentity>,
    previous_index: Option<usize>,
) -> Option<usize> {
    let file = file?;
    if file.hunks.is_empty() {
        return None;
    }

    if !kept_file_selection {
        return Some(0);
    }

    if let Some(index) = previous_identity.and_then(|identity| find_hunk_index(file, identity)) {
        return Some(index);
    }

    if let Some(index) = previous_index {
        return Some(index.min(file.hunks.len() - 1));
    }

    Some(0)
}

fn find_hunk_index(file: &DiffFile, identity: HunkIdentity) -> Option<usize> {
    file.hunks
        .iter()
        .position(|hunk| hunk_identity(hunk) == identity)
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
    use crate::model::{DiffLine, DiffLineKind, FileStage, FileStatus, SourceSnapshot};

    #[test]
    fn replacement_preserves_file_hunk_scroll_and_folder_by_identity() {
        let current = changeset(vec![
            file("src/a.rs", vec![hunk(1, 1, 1, 1)]),
            file("src/b.rs", vec![hunk(4, 1, 8, 1), hunk(20, 2, 30, 2)]),
        ]);
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[1]),
            1,
            Some(&current.files[1].hunks[1]),
            Some(1),
            12,
            Some(SidebarRowTarget::Folder("src".to_string())),
        );

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![
                file("src/b.rs", vec![hunk(4, 1, 8, 1), hunk(20, 2, 30, 2)]),
                file("src/a.rs", vec![hunk(1, 1, 1, 1)]),
            ]),
            true,
        );

        assert_eq!(replacement.selected_file_index, 0);
        assert_eq!(replacement.selected_hunk_index, Some(1));
        assert_eq!(replacement.diff_scroll, 12);
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
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[1]),
            1,
            Some(&current.files[1].hunks[0]),
            Some(0),
            8,
            None,
        );

        let replacement =
            ReviewSessionReplacement::plan(snapshot, changeset(vec![file("a.rs", vec![])]), true);

        assert_eq!(replacement.selected_file_index, 0);
        assert_eq!(replacement.selected_hunk_index, None);
        assert_eq!(replacement.diff_scroll, 0);
    }

    #[test]
    fn replacement_clamps_hunk_index_when_coordinates_disappear() {
        let current = changeset(vec![file(
            "sample.rs",
            vec![hunk(1, 1, 1, 1), hunk(10, 1, 10, 1), hunk(20, 1, 20, 1)],
        )]);
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[0]),
            0,
            Some(&current.files[0].hunks[2]),
            Some(2),
            6,
            None,
        );

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![file("sample.rs", vec![hunk(1, 1, 1, 1)])]),
            true,
        );

        assert_eq!(replacement.selected_file_index, 0);
        assert_eq!(replacement.selected_hunk_index, Some(0));
        assert_eq!(replacement.diff_scroll, 6);
    }

    #[test]
    fn replacement_drops_folder_target_when_folder_disappears() {
        let current = changeset(vec![file("src/a.rs", vec![hunk(1, 1, 1, 1)])]);
        let snapshot = ReviewSessionSnapshot::capture(
            Some(&current.files[0]),
            0,
            Some(&current.files[0].hunks[0]),
            Some(0),
            2,
            Some(SidebarRowTarget::Folder("src".to_string())),
        );

        let replacement = ReviewSessionReplacement::plan(
            snapshot,
            changeset(vec![file("tests/a.rs", vec![hunk(1, 1, 1, 1)])]),
            true,
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
