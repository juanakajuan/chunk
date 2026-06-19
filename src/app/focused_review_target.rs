use crate::model::DiffFile;

use super::FocusPane;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FocusedReviewTargetError {
    message: &'static str,
}

impl FocusedReviewTargetError {
    fn new(message: &'static str) -> Self {
        Self { message }
    }

    pub(super) fn message(self) -> &'static str {
        self.message
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SelectedFile<'a> {
    pub(super) file_index: usize,
    pub(super) file: &'a DiffFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SelectedHunk<'a> {
    pub(super) file_index: usize,
    pub(super) file: &'a DiffFile,
    pub(super) hunk_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AskAiFocusedTarget<'a> {
    pub(super) file: &'a DiffFile,
    pub(super) hunk_index: Option<usize>,
    pub(super) selected_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FocusedCopyTarget<'a> {
    FilePath(SelectedFile<'a>),
    HunkDiff(SelectedHunk<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FocusedMutationTarget<'a> {
    File(SelectedFile<'a>),
    Hunk(SelectedHunk<'a>),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FocusedReviewTarget<'a> {
    files: &'a [DiffFile],
    selected_file_index: usize,
    selected_hunk_index: Option<usize>,
    focus: FocusPane,
    files_panel_visible: bool,
}

impl<'a> FocusedReviewTarget<'a> {
    pub(super) fn new(
        files: &'a [DiffFile],
        selected_file_index: usize,
        selected_hunk_index: Option<usize>,
        focus: FocusPane,
        files_panel_visible: bool,
    ) -> Self {
        Self {
            files,
            selected_file_index,
            selected_hunk_index,
            focus,
            files_panel_visible,
        }
    }

    pub(super) fn stage_keybind_hint(self) -> Option<&'static str> {
        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => Some("stage file"),
            FocusPane::Diff => Some("stage hunk"),
            FocusPane::Sidebar => None,
        }
    }

    pub(super) fn ask_ai(
        self,
        selected_text: Option<String>,
        missing_file_message: &'static str,
    ) -> Result<AskAiFocusedTarget<'a>, FocusedReviewTargetError> {
        let file = self.require_selected_file(missing_file_message)?;
        let file_context = self.focus == FocusPane::Sidebar && self.files_panel_visible;

        Ok(AskAiFocusedTarget {
            file: file.file,
            hunk_index: if file_context {
                None
            } else {
                self.selected_hunk_index
            },
            selected_text: if file_context { None } else { selected_text },
        })
    }

    pub(super) fn copy(self) -> Result<FocusedCopyTarget<'a>, FocusedReviewTargetError> {
        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => self
                .require_selected_file("no selected file path to copy")
                .map(FocusedCopyTarget::FilePath),
            FocusPane::Diff => self
                .require_selected_hunk("no selected hunk to copy")
                .map(FocusedCopyTarget::HunkDiff),
            FocusPane::Sidebar => Err(FocusedReviewTargetError::new(
                "no visible file path to copy",
            )),
        }
    }

    pub(super) fn file_diff_copy(self) -> Result<SelectedFile<'a>, FocusedReviewTargetError> {
        self.require_selected_file("no selected file to copy")
    }

    pub(super) fn staging(
        self,
    ) -> Result<Option<FocusedMutationTarget<'a>>, FocusedReviewTargetError> {
        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => {
                Ok(self.selected_file().map(FocusedMutationTarget::File))
            }
            FocusPane::Diff => self
                .require_selected_hunk("no selected hunk to stage")
                .map(FocusedMutationTarget::Hunk)
                .map(Some),
            FocusPane::Sidebar => Ok(None),
        }
    }

    pub(super) fn discard(
        self,
    ) -> Result<Option<FocusedMutationTarget<'a>>, FocusedReviewTargetError> {
        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => self
                .require_selected_file("no selected file to discard")
                .map(FocusedMutationTarget::File)
                .map(Some),
            FocusPane::Diff => self
                .require_selected_hunk("no selected hunk to discard")
                .map(FocusedMutationTarget::Hunk)
                .map(Some),
            FocusPane::Sidebar => Ok(None),
        }
    }

    pub(super) fn editor(self) -> Result<SelectedFile<'a>, FocusedReviewTargetError> {
        self.require_selected_file("no selected file to open")
    }

    fn selected_file(self) -> Option<SelectedFile<'a>> {
        self.files
            .get(self.selected_file_index)
            .map(|file| SelectedFile {
                file_index: self.selected_file_index,
                file,
            })
    }

    fn require_selected_file(
        self,
        message: &'static str,
    ) -> Result<SelectedFile<'a>, FocusedReviewTargetError> {
        self.selected_file()
            .ok_or_else(|| FocusedReviewTargetError::new(message))
    }

    fn require_selected_hunk(
        self,
        message: &'static str,
    ) -> Result<SelectedHunk<'a>, FocusedReviewTargetError> {
        let file = self
            .selected_file()
            .ok_or_else(|| FocusedReviewTargetError::new(message))?;
        let hunk_index = self
            .selected_hunk_index
            .filter(|index| *index < file.file.hunks.len())
            .ok_or_else(|| FocusedReviewTargetError::new(message))?;

        Ok(SelectedHunk {
            file_index: file.file_index,
            file: file.file,
            hunk_index,
        })
    }
}
