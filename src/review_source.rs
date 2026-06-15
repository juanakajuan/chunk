//! Review-source module for worktree and pull-request diffs.
//!
//! This module owns source-specific behavior: loading, live reload capability,
//! staging, source snapshots, and empty-state messages. The app session handles
//! selection and scroll; callers do not need to know which Git commands back a
//! source.

use std::path::{Component, Path, PathBuf};

use color_eyre::eyre::{Result, eyre};

use crate::ask_ai::AskAiReviewMode;
use crate::editor::EditorRequest;
use crate::git;
use crate::model::{Changeset, DiffFile, FileStatus};

const NO_TRACKED_CHANGES: &str = "No tracked changes";
const NO_DIFF_MESSAGE: &str = "No diff to review. Make a tracked change, then run chunk diff.";
const NO_BRANCH_CHANGES: &str = "No branch changes";
const NO_PR_DIFF_MESSAGE: &str = "No diff to review. Current branch has no changes against base.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedReview {
    pub(crate) source: ReviewSource,
    pub(crate) changeset: Changeset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewSource {
    Worktree(WorktreeReviewSource),
    PullRequest(PullRequestReviewSource),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorktreeReviewSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequestReviewSource {
    old_ref: String,
    new_ref: String,
    title: String,
    source_label: String,
}

impl LoadedReview {
    #[cfg(test)]
    pub(crate) fn worktree(changeset: Changeset) -> Self {
        Self {
            source: ReviewSource::worktree(),
            changeset,
        }
    }
}

impl ReviewSource {
    pub(crate) fn load_worktree() -> Result<LoadedReview> {
        let source = Self::worktree();
        let changeset = source.reload()?;
        Ok(LoadedReview { source, changeset })
    }

    pub(crate) fn load_pull_request(base: Option<&str>) -> Result<LoadedReview> {
        let diff = git::load_pr_diff(base)?;
        let source = Self::PullRequest(PullRequestReviewSource {
            old_ref: diff.old_ref,
            new_ref: diff.new_ref,
            title: diff.changeset.title.clone(),
            source_label: diff.changeset.source_label.clone(),
        });

        Ok(LoadedReview {
            source,
            changeset: diff.changeset,
        })
    }

    pub(crate) fn can_stage(&self) -> bool {
        matches!(self, Self::Worktree(_))
    }

    pub(crate) fn can_discard(&self) -> bool {
        matches!(self, Self::Worktree(_))
    }

    pub(crate) fn live_watch_root(&self) -> Result<Option<PathBuf>> {
        match self {
            Self::Worktree(source) => source.live_watch_root().map(Some),
            Self::PullRequest(_) => Ok(None),
        }
    }

    pub(crate) fn reload(&self) -> Result<Changeset> {
        match self {
            Self::Worktree(source) => source.reload(),
            Self::PullRequest(source) => source.reload(),
        }
    }

    pub(crate) fn load_source_snapshots(&self, file: &mut DiffFile) {
        match self {
            Self::Worktree(source) => source.load_source_snapshots(file),
            Self::PullRequest(source) => source.load_source_snapshots(file),
        }
    }

    pub(crate) fn toggle_staging_for_file(&self, path: &str) -> Result<Option<Changeset>> {
        match self {
            Self::Worktree(source) => source.toggle_staging_for_file(path).map(Some),
            Self::PullRequest(_) => Ok(None),
        }
    }

    pub(crate) fn toggle_staging_for_hunk(
        &self,
        file: &DiffFile,
        hunk_index: usize,
    ) -> Result<Option<Changeset>> {
        match self {
            Self::Worktree(source) => source.toggle_staging_for_hunk(file, hunk_index).map(Some),
            Self::PullRequest(_) => Ok(None),
        }
    }

    pub(crate) fn discard_file(&self, path: &str) -> Result<Option<Changeset>> {
        match self {
            Self::Worktree(source) => source.discard_file(path).map(Some),
            Self::PullRequest(_) => Ok(None),
        }
    }

    pub(crate) fn discard_hunk(
        &self,
        file: &DiffFile,
        hunk_index: usize,
    ) -> Result<Option<Changeset>> {
        match self {
            Self::Worktree(source) => source.discard_hunk(file, hunk_index).map(Some),
            Self::PullRequest(_) => Ok(None),
        }
    }

    pub(crate) fn editor_request(&self, file: &DiffFile) -> Result<EditorRequest> {
        match self {
            Self::Worktree(source) => source.editor_request(file),
            Self::PullRequest(_) => Err(eyre!(
                "cannot open PR snapshot in editor; run `chunk diff` to edit worktree files"
            )),
        }
    }

    pub(crate) fn empty_sidebar_message(&self) -> &'static str {
        match self {
            Self::Worktree(_) => NO_TRACKED_CHANGES,
            Self::PullRequest(_) => NO_BRANCH_CHANGES,
        }
    }

    pub(crate) fn no_diff_message(&self) -> &'static str {
        match self {
            Self::Worktree(_) => NO_DIFF_MESSAGE,
            Self::PullRequest(_) => NO_PR_DIFF_MESSAGE,
        }
    }

    pub(crate) fn ask_ai_review_mode(&self) -> AskAiReviewMode {
        match self {
            Self::Worktree(_) => AskAiReviewMode::Worktree,
            Self::PullRequest(_) => AskAiReviewMode::PullRequest,
        }
    }

    fn worktree() -> Self {
        Self::Worktree(WorktreeReviewSource)
    }
}

impl WorktreeReviewSource {
    fn live_watch_root(self) -> Result<PathBuf> {
        git::worktree_root()
    }

    fn reload(self) -> Result<Changeset> {
        git::load_worktree_diff()
    }

    fn load_source_snapshots(self, file: &mut DiffFile) {
        git::load_worktree_source_snapshots(file);
    }

    fn toggle_staging_for_file(self, path: &str) -> Result<Changeset> {
        git::toggle_staging_for_file(path)?;
        git::load_worktree_diff()
    }

    fn toggle_staging_for_hunk(self, file: &DiffFile, hunk_index: usize) -> Result<Changeset> {
        git::toggle_staging_for_hunk(file, hunk_index)?;
        git::load_worktree_diff()
    }

    fn discard_file(self, path: &str) -> Result<Changeset> {
        git::discard_worktree_file(path)?;
        git::load_worktree_diff()
    }

    fn discard_hunk(self, file: &DiffFile, hunk_index: usize) -> Result<Changeset> {
        git::discard_worktree_hunk(file, hunk_index)?;
        git::load_worktree_diff()
    }

    fn editor_request(self, file: &DiffFile) -> Result<EditorRequest> {
        let path = editable_file_path(file)?;
        let root = git::worktree_root()?;

        Ok(EditorRequest {
            path: worktree_file_path(&root, path)?,
            line: file.first_changed_line(),
        })
    }
}

impl PullRequestReviewSource {
    fn reload(&self) -> Result<Changeset> {
        let mut changeset = git::load_ref_diff(&self.old_ref, &self.new_ref)?;
        changeset.title.clone_from(&self.title);
        changeset.source_label.clone_from(&self.source_label);
        Ok(changeset)
    }

    fn load_source_snapshots(&self, file: &mut DiffFile) {
        git::load_ref_source_snapshots(file, &self.old_ref, &self.new_ref);
    }
}

fn editable_file_path(file: &DiffFile) -> Result<&str> {
    if file.status == FileStatus::Deleted {
        return Err(eyre!(
            "cannot open deleted file in editor: {}",
            file.display_path()
        ));
    }

    if file.path.is_empty() {
        return Err(eyre!("selected file has no worktree path to open"));
    }

    Ok(&file.path)
}

fn worktree_file_path(root: &Path, path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute() || relative.components().any(escapes_worktree) {
        return Err(eyre!("cannot open path outside worktree: {path}"));
    }

    Ok(root.join(relative))
}

fn escapes_worktree(component: Component<'_>) -> bool {
    matches!(
        component,
        Component::ParentDir | Component::RootDir | Component::Prefix(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiffHunk, DiffLine, DiffLineKind, FileStage, SourceSnapshot};

    #[test]
    fn pull_request_source_is_read_only_and_not_live_watched() {
        let source = pull_request_source();
        let file = diff_file(FileStatus::Modified, "src/lib.rs", "src/lib.rs");

        assert!(!source.can_stage());
        assert!(!source.can_discard());
        assert_eq!(source.live_watch_root().unwrap(), None);
        assert_eq!(source.toggle_staging_for_file("src/lib.rs").unwrap(), None);
        assert_eq!(source.toggle_staging_for_hunk(&file, 0).unwrap(), None);
        assert_eq!(source.discard_file("src/lib.rs").unwrap(), None);
        assert_eq!(source.discard_hunk(&file, 0).unwrap(), None);
        assert!(
            source
                .editor_request(&file)
                .unwrap_err()
                .to_string()
                .contains("cannot open PR snapshot")
        );
        assert_eq!(source.empty_sidebar_message(), NO_BRANCH_CHANGES);
        assert_eq!(source.no_diff_message(), NO_PR_DIFF_MESSAGE);
        assert_eq!(source.ask_ai_review_mode(), AskAiReviewMode::PullRequest);
    }

    #[test]
    fn worktree_source_exposes_worktree_affordances() {
        let source = ReviewSource::worktree();

        assert!(source.can_stage());
        assert!(source.can_discard());
        assert_eq!(source.empty_sidebar_message(), NO_TRACKED_CHANGES);
        assert_eq!(source.no_diff_message(), NO_DIFF_MESSAGE);
        assert_eq!(source.ask_ai_review_mode(), AskAiReviewMode::Worktree);
    }

    #[test]
    fn editable_file_path_rejects_files_that_cannot_be_opened() {
        let valid = diff_file(FileStatus::Modified, "src/lib.rs", "src/lib.rs");
        assert_eq!(editable_file_path(&valid).unwrap(), "src/lib.rs");

        let deleted = diff_file(FileStatus::Deleted, "src/lib.rs", "");
        assert!(
            editable_file_path(&deleted)
                .unwrap_err()
                .to_string()
                .contains("cannot open deleted file")
        );

        let empty = diff_file(FileStatus::Modified, "", "");
        assert!(
            editable_file_path(&empty)
                .unwrap_err()
                .to_string()
                .contains("no worktree path")
        );
    }

    #[test]
    fn worktree_file_path_rejects_paths_outside_root() {
        let root = Path::new("/repo");

        assert_eq!(
            worktree_file_path(root, "src/lib.rs").unwrap(),
            PathBuf::from("/repo/src/lib.rs")
        );
        for path in ["/tmp/lib.rs", "../lib.rs", "src/../lib.rs"] {
            assert!(
                worktree_file_path(root, path)
                    .unwrap_err()
                    .to_string()
                    .contains("outside worktree"),
                "{path} should be rejected"
            );
        }
    }

    fn pull_request_source() -> ReviewSource {
        ReviewSource::PullRequest(PullRequestReviewSource {
            old_ref: "main".to_string(),
            new_ref: "HEAD".to_string(),
            title: "PR review feature into main".to_string(),
            source_label: "git diff main...HEAD".to_string(),
        })
    }

    fn diff_file(status: FileStatus, old_path: &str, path: &str) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: old_path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status,
            stage: FileStage::Unstaged,
            additions: 1,
            deletions: 1,
            hunks: vec![DiffHunk {
                header: "@@ -10,1 +10,1 @@".to_string(),
                old_start: 10,
                old_lines: 1,
                new_start: 10,
                new_lines: 1,
                stage: FileStage::Unstaged,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(10),
                    content: "new line".to_string(),
                }],
            }],
            binary: false,
        }
    }
}
