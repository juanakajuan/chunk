//! Terminal application session state.
//!
//! `App` owns global selection, focus, overlays, and live reload errors. The
//! selected review diff owns its navigation, rendering, search, and geometry;
//! terminal/watch orchestration lives in `runtime`, while `ui` draws Ratatui
//! widgets.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::ask_ai::{AiSettings, AskAiContext, AskAiRequest, AskAiResult};
use crate::config::AppConfig;
use crate::custom_command::{CustomCommandBinding, CustomCommandResult};
use crate::editor::EditorRequest;
use crate::keybind::{BuiltinAction, KeybindMap};
use crate::model::{Changeset, DiffFile, FileStage, RepositoryOverview};
use crate::patch;
use crate::review_source::{LoadedReview, ReviewSource, WorktreeMutation};
use crate::rows;
use crate::scroll_text::VerticalDirection;
use crate::selection::TextSelection;
use crate::theme::{Theme, ThemeName};
use crate::viewport::DiffScrollbar;

mod diff_frame;
mod files_panel;
mod focused_review_target;
mod keys;
mod overlay;
mod repository_review;
mod selected_diff;

pub(crate) use keys::accepts_text_input;
pub(crate) use overlay::AiPickerField;
pub(crate) use repository_review::{CommitDetailsOutcome, RepositoryPane};

use files_panel::{FilesPanel, FilesPanelRender, FilesPanelTarget};
use focused_review_target::{FocusedCopyTarget, FocusedMutationTarget, FocusedReviewTarget};
use keys::is_ctrl_c;
use overlay::{AskAiPromptState, DiscardConfirmation, DiscardTarget, Overlay};
use repository_review::{
    RepositoryPanelRender, RepositoryPanelTarget, RepositoryReview, RepositoryReviewIntent,
};
use selected_diff::SelectedDiffPane;

const MOUSE_WHEEL_STEP: usize = 3;
const HELP_OVERLAY_SCROLL_PAGE: usize = 8;
const LIVE_NOTICE_DURATION: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Sidebar,
    Branches,
    Commits,
    Diff,
}

impl FocusPane {
    fn repository_pane(self) -> Option<RepositoryPane> {
        match self {
            Self::Branches => Some(RepositoryPane::Branches),
            Self::Commits => Some(RepositoryPane::Commits),
            Self::Sidebar | Self::Diff => None,
        }
    }
}

impl RepositoryPane {
    pub(crate) fn focus(self) -> FocusPane {
        match self {
            Self::Branches => FocusPane::Branches,
            Self::Commits => FocusPane::Commits,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingLeftClick {
    Sidebar { target: FilesPanelTarget },
    Repository { target: RepositoryPanelTarget },
    Diff { hunk_index: Option<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalAiAction {
    OpenHarnessPicker,
    OpenPrompt,
    ExplainCode,
    SummarizeUnpublished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StagingRequest {
    File {
        path: String,
    },
    Folder {
        path: String,
        file_paths: Vec<String>,
        action: FolderStagingAction,
    },
    Hunk {
        file: DiffFile,
        hunk_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderStagingAction {
    Stage,
    Unstage,
}

impl StagingRequest {
    fn into_mutation(self) -> WorktreeMutation {
        match self {
            Self::File { path } => WorktreeMutation::ToggleFileStaging { path },
            Self::Folder {
                file_paths, action, ..
            } => match action {
                FolderStagingAction::Stage => WorktreeMutation::StageFiles { paths: file_paths },
                FolderStagingAction::Unstage => {
                    WorktreeMutation::UnstageFiles { paths: file_paths }
                }
            },
            Self::Hunk { file, hunk_index } => {
                WorktreeMutation::ToggleHunkStaging { file, hunk_index }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardRequest {
    text: String,
    success_message: String,
}

impl ClipboardRequest {
    fn new(text: String, success_message: impl Into<String>) -> Self {
        Self {
            text,
            success_message: success_message.into(),
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn success_message(&self) -> &str {
        &self.success_message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppEffect {
    LoadCommitDetails {
        revision: String,
    },
    CancelCommitDetailsLoad,
    OpenEditor(EditorRequest),
    RunCustomCommand(CustomCommandBinding),
    CancelCustomCommand,
    RunAskAi {
        settings: Box<AiSettings>,
        request: AskAiRequest,
    },
    RunUnpublishedSummary {
        settings: Box<AiSettings>,
    },
    CancelAskAi,
    SaveAiSettings(Box<AiSettings>),
    CopyToClipboard(ClipboardRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorOutcome {
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardWriteResult {
    request: ClipboardRequest,
    error: Option<String>,
}

impl ClipboardWriteResult {
    pub(crate) fn completed(request: ClipboardRequest) -> Self {
        Self {
            request,
            error: None,
        }
    }

    pub(crate) fn failed(request: ClipboardRequest, error: impl Into<String>) -> Self {
        Self {
            request,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEvent {
    CommitDetailsFinished {
        revision: String,
        outcome: CommitDetailsOutcome,
    },
    EditorFinished(EditorOutcome),
    CustomCommandFinished(CustomCommandResult),
    AskAiFinished(AskAiResult),
    AiSettingsSaved {
        settings: AiSettings,
        error: Option<String>,
    },
    ClipboardWriteFinished(ClipboardWriteResult),
    LiveWatchFailed(String),
    ReloadReviewSource {
        preserve_scroll: bool,
    },
}

#[derive(Debug)]
pub(crate) struct App {
    /// Review source behavior for this session.
    source: ReviewSource,
    /// Current diff data being reviewed.
    changeset: Changeset,
    /// Read-only repository selection and selected commit-details lifecycle.
    repository_review: RepositoryReview,
    /// Last live reload/watch error, rendered above the diff when present.
    live_error: Option<String>,
    /// Transient successful background/status event rendered above the diff.
    live_notice: Option<String>,
    live_notice_expires_at: Option<Instant>,
    /// Changed-files tree, including selected file, cursor, and panel geometry.
    files_panel: FilesPanel,
    /// Selected review diff, including navigation, search, rendering, and geometry.
    selected_diff: SelectedDiffPane,
    /// Pane receiving keyboard and mouse wheel actions.
    focus: FocusPane,
    /// Whether the files sidebar is visible in the current session.
    files_panel_visible: bool,
    /// Active modal overlay (help, discard confirmation, running command, or
    /// command output); at most one at a time.
    overlay: Option<Overlay>,
    /// User-configured shell command bindings.
    custom_commands: Vec<CustomCommandBinding>,
    /// User-configured built-in keybindings.
    keybinds: KeybindMap,
    /// User-selected UI and syntax palette.
    theme: ThemeName,
    /// AI CLI used for Ask AI requests.
    ai_settings: AiSettings,
    /// Deferred runtime effects requested by application behavior.
    effects: VecDeque<AppEffect>,
    /// Visible text rows and active drag-to-copy selection.
    text_selection: TextSelection,
    /// Deferred plain click target, cancelled if the press becomes a drag.
    pending_left_click: Option<PendingLeftClick>,
}

pub(crate) struct DiffPaneRows {
    pub(crate) title: String,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) scrollbar: Option<DiffScrollbar>,
}

impl App {
    #[cfg(test)]
    pub(crate) fn new(review: LoadedReview) -> Self {
        Self::with_config(review, AppConfig::default())
    }

    pub(crate) fn with_config(review: LoadedReview, config: AppConfig) -> Self {
        let LoadedReview {
            source,
            changeset,
            repository_overview,
        } = review;
        let selected_diff = SelectedDiffPane::new(&changeset);
        let files_panel = FilesPanel::new(&changeset.files);
        let repository_review = RepositoryReview::new(repository_overview);
        Self {
            source,
            changeset,
            repository_review,
            live_error: None,
            live_notice: None,
            live_notice_expires_at: None,
            files_panel,
            selected_diff,
            focus: FocusPane::Sidebar,
            files_panel_visible: true,
            overlay: None,
            custom_commands: config.commands,
            keybinds: config.keybinds,
            theme: config.theme,
            ai_settings: config.ai,
            effects: VecDeque::new(),
            text_selection: TextSelection::default(),
            pending_left_click: None,
        }
    }

    pub(crate) fn begin_render_frame(&mut self) {
        self.repository_review.invalidate_frame();
        self.files_panel.invalidate_frame();
        self.selected_diff.invalidate_frame();
        self.text_selection.begin_frame();
    }

    pub(crate) fn files_panel_visible(&self) -> bool {
        self.files_panel_visible
    }

    pub(crate) fn focus(&self) -> FocusPane {
        self.focus
    }

    pub(crate) fn theme(&self) -> Theme {
        self.theme.theme()
    }

    pub(crate) fn keybinds(&self) -> KeybindMap {
        self.keybinds
    }

    pub(crate) fn sidebar_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.ensure_scroll_bounds();
        let lines = self.files_panel.render(FilesPanelRender {
            area,
            files: &self.changeset.files,
            empty_message: self.empty_sidebar_message(),
            content_width,
            visible_height,
            theme,
            focused: self.focus == FocusPane::Sidebar,
        });
        self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        )
    }

    pub(crate) fn repository_panel_rows(
        &mut self,
        pane: RepositoryPane,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let lines = self.repository_review.render(RepositoryPanelRender {
            pane,
            area,
            content_width,
            visible_height,
            theme,
            focused: self.focus == pane.focus(),
        });
        self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        )
    }

    pub(crate) fn selectable_lines(
        &mut self,
        area: Rect,
        lines: Vec<Line<'static>>,
        line_scroll: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.text_selection
            .decorate_visible_lines(area, lines, line_scroll, visible_height, theme)
    }

    pub(crate) fn keybind_bar_line(&self, theme: Theme) -> Line<'static> {
        if self.ask_ai_output().is_some() {
            return rows::ask_ai_output_keybind_bar_line(self.keybinds, theme);
        }
        if self.ask_ai_prompt().is_some() {
            return rows::ask_ai_prompt_keybind_bar_line(theme);
        }
        if self.ask_ai_running().is_some() {
            return rows::ask_ai_running_keybind_bar_line(self.keybinds, theme);
        }
        if self.command_running().is_some() {
            return rows::custom_command_running_keybind_bar_line(self.keybinds, theme);
        }
        if self.command_output().is_some() {
            return rows::custom_command_output_keybind_bar_line(self.keybinds, theme);
        }

        rows::keybind_bar_line(
            self.files_panel_visible,
            self.stage_keybind_hint(),
            self.repository_copy_keybind_hint(),
            self.keybinds,
            theme,
        )
    }

    pub(crate) fn keybind_mode_tag_line(&self, theme: Theme) -> Option<Line<'static>> {
        if self.ask_ai_output().is_some()
            || self.ask_ai_prompt().is_some()
            || self.ask_ai_running().is_some()
            || self.command_output().is_some()
        {
            return None;
        }

        Some(rows::keybind_mode_tag_line(self.can_stage(), theme))
    }

    pub(crate) fn help_overlay_lines(
        &self,
        content_width: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        rows::help_overlay_lines(
            self.can_stage(),
            self.can_discard(),
            self.ai_settings.harness,
            &self.custom_commands,
            self.keybinds,
            content_width,
            theme,
        )
    }

    pub(crate) fn harness_picker_lines(
        &self,
        content_width: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let (selected, field) = match &self.overlay {
            Some(Overlay::HarnessPicker(picker)) => (&picker.settings, picker.field),
            _ => (&self.ai_settings, AiPickerField::Harness),
        };
        rows::harness_picker_lines(&self.ai_settings, selected, field, content_width, theme)
    }

    fn can_stage(&self) -> bool {
        !self.repository_review.details_active() && self.source.worktree_mutations().is_some()
    }

    fn can_discard(&self) -> bool {
        !self.repository_review.details_active() && self.source.worktree_mutations().is_some()
    }

    fn stage_keybind_hint(&self) -> Option<&'static str> {
        if !self.can_stage() {
            return None;
        }

        self.focused_review_target().stage_keybind_hint()
    }

    fn repository_copy_keybind_hint(&self) -> Option<&'static str> {
        self.focus
            .repository_pane()
            .map(RepositoryPane::copy_keybind_hint)
    }

    pub(crate) fn live_watch_root(&self) -> Result<Option<PathBuf>> {
        self.source.live_watch_root()
    }

    fn empty_sidebar_message(&self) -> &'static str {
        self.source.empty_sidebar_message()
    }

    fn no_diff_message(&self) -> &'static str {
        self.source.no_diff_message()
    }

    fn selected_file(&self) -> Option<&DiffFile> {
        self.changeset.files.get(self.selected_file_index())
    }

    fn selected_file_index(&self) -> usize {
        self.files_panel.selected_file_index()
    }

    fn focused_review_target(&self) -> FocusedReviewTarget<'_> {
        let focus = if self.repository_review.details_active() {
            FocusPane::Commits
        } else {
            self.focus
        };
        FocusedReviewTarget::new(
            &self.changeset.files,
            self.selected_file_index(),
            self.selected_diff.selected_hunk_index(),
            focus,
            self.files_panel_visible,
        )
    }

    fn ensure_scroll_bounds(&mut self) {
        let selected_file_index = self.selected_file_index();
        let selected_file = self.changeset.files.get(selected_file_index);
        self.selected_diff
            .ensure_bounds(selected_file_index, selected_file);
    }

    pub(crate) fn set_live_error(&mut self, error: String) {
        self.clear_live_notice();
        self.live_error = Some(error);
    }

    pub(crate) fn set_live_notice(&mut self, notice: String) {
        self.live_error = None;
        self.live_notice = Some(notice);
        self.live_notice_expires_at = Some(Instant::now() + LIVE_NOTICE_DURATION);
    }

    pub(crate) fn expire_live_notice(&mut self) {
        self.expire_live_notice_at(Instant::now());
    }

    fn expire_live_notice_at(&mut self, now: Instant) {
        if self
            .live_notice_expires_at
            .is_some_and(|expires_at| now >= expires_at)
        {
            self.clear_live_notice();
        }
    }

    fn clear_live_notice(&mut self) {
        self.live_notice = None;
        self.live_notice_expires_at = None;
    }

    pub(crate) fn handle_runtime_event(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::CommitDetailsFinished { revision, outcome } => self
                .repository_review
                .apply_commit_details(&revision, outcome),
            RuntimeEvent::EditorFinished(EditorOutcome::Completed) => {
                self.reload_review_source(true);
            }
            RuntimeEvent::EditorFinished(EditorOutcome::Failed(error)) => {
                self.set_live_error(error);
            }
            RuntimeEvent::CustomCommandFinished(result) => {
                self.set_custom_command_result(result);
                self.reload_review_source(true);
            }
            RuntimeEvent::AskAiFinished(result) => {
                self.set_ask_ai_result(result);
            }
            RuntimeEvent::AiSettingsSaved { settings, error } => {
                if let Some(error) = error {
                    self.set_live_error(format!(
                        "failed to save AI settings for {}: {error}",
                        settings.harness.display_name()
                    ));
                } else {
                    self.set_live_notice(format!(
                        "AI settings saved: {}",
                        settings.harness.display_name()
                    ));
                }
            }
            RuntimeEvent::ClipboardWriteFinished(result) => {
                if let Some(error) = result.error {
                    self.set_live_error(format!("copy failed: {error}"));
                } else {
                    self.set_live_notice(result.request.success_message().to_string());
                }
            }
            RuntimeEvent::LiveWatchFailed(error) => {
                self.set_live_error(format!("watch failed: {error}"));
            }
            RuntimeEvent::ReloadReviewSource { preserve_scroll } => {
                self.reload_review_source(preserve_scroll);
            }
        }
    }

    pub(crate) fn take_effects(&mut self) -> Vec<AppEffect> {
        let has_clipboard_effect = self
            .effects
            .iter()
            .any(|effect| matches!(effect, AppEffect::CopyToClipboard(_)));
        let mut effects = self.effects.drain(..).collect::<Vec<_>>();

        if !has_clipboard_effect && let Some(text) = self.text_selection.take_clipboard_request() {
            effects.push(AppEffect::CopyToClipboard(ClipboardRequest::new(
                text,
                "copied selected text",
            )));
        }

        effects
    }

    fn queue_editor_effect(&mut self, request: EditorRequest) {
        self.clear_effects(|effect| matches!(effect, AppEffect::OpenEditor(_)));
        self.effects.push_back(AppEffect::OpenEditor(request));
    }

    fn apply_repository_review_intent(&mut self, intent: Option<RepositoryReviewIntent>) {
        let Some(intent) = intent else {
            return;
        };
        match intent {
            RepositoryReviewIntent::LoadCommitDetails { revision } => {
                self.clear_effects(|effect| {
                    matches!(
                        effect,
                        AppEffect::LoadCommitDetails { .. } | AppEffect::CancelCommitDetailsLoad
                    )
                });
                self.effects
                    .push_back(AppEffect::LoadCommitDetails { revision });
            }
            RepositoryReviewIntent::KeepCommitDetailsLoad => {
                self.clear_effects(|effect| matches!(effect, AppEffect::CancelCommitDetailsLoad));
            }
            RepositoryReviewIntent::ClearCommitDetailsLoad => {
                self.clear_effects(|effect| matches!(effect, AppEffect::LoadCommitDetails { .. }));
            }
            RepositoryReviewIntent::CancelCommitDetailsLoad => {
                self.clear_effects(|effect| {
                    matches!(
                        effect,
                        AppEffect::LoadCommitDetails { .. } | AppEffect::CancelCommitDetailsLoad
                    )
                });
                self.effects.push_back(AppEffect::CancelCommitDetailsLoad);
            }
        }
    }

    fn set_focus(&mut self, focus: FocusPane) {
        let changed = self.focus != focus;
        self.focus = focus;
        let intent = self.repository_review.focus_changed(focus, changed);
        self.apply_repository_review_intent(intent);
    }

    fn queue_custom_command_effect(&mut self, command: CustomCommandBinding) {
        self.clear_effects(|effect| matches!(effect, AppEffect::RunCustomCommand(_)));
        self.set_custom_command_running(&command);
        self.effects.push_back(AppEffect::RunCustomCommand(command));
    }

    fn queue_custom_command_cancel_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::CancelCustomCommand));
        self.effects.push_back(AppEffect::CancelCustomCommand);
    }

    fn queue_ask_ai_request_effect(&mut self, request: AskAiRequest) {
        self.clear_effects(|effect| matches!(effect, AppEffect::RunAskAi { .. }));
        self.set_ask_ai_running(&request);
        self.effects.push_back(AppEffect::RunAskAi {
            settings: Box::new(self.ai_settings.clone()),
            request,
        });
    }

    fn queue_unpublished_summary_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::RunUnpublishedSummary { .. }));
        self.set_ask_ai_running_question(crate::ask_ai::UNPUBLISHED_SUMMARY_QUESTION);
        self.effects.push_back(AppEffect::RunUnpublishedSummary {
            settings: Box::new(self.ai_settings.clone()),
        });
    }

    fn queue_ai_settings_save_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::SaveAiSettings(_)));
        self.effects.push_back(AppEffect::SaveAiSettings(Box::new(
            self.ai_settings.clone(),
        )));
    }

    fn queue_ask_ai_cancel_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::CancelAskAi));
        self.effects.push_back(AppEffect::CancelAskAi);
    }

    fn queue_clipboard_effect(&mut self, request: ClipboardRequest) {
        self.clear_effects(|effect| matches!(effect, AppEffect::CopyToClipboard(_)));
        self.effects.push_back(AppEffect::CopyToClipboard(request));
    }

    fn clear_clipboard_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::CopyToClipboard(_)));
    }

    fn clear_effects(&mut self, mut should_clear: impl FnMut(&AppEffect) -> bool) {
        self.effects.retain(|effect| !should_clear(effect));
    }

    pub(crate) fn reload_review_source(&mut self, preserve_scroll: bool) {
        match self.source.reload() {
            Ok(changeset) => {
                self.apply_reloaded_changeset(changeset, preserve_scroll);
                self.reload_repository_overview();
            }
            Err(error) => self.live_error = Some(format!("reload failed: {error}")),
        }
    }

    fn reload_repository_overview(&mut self) {
        match self.source.repository_overview() {
            Ok(repository_overview) => self.apply_repository_overview(repository_overview),
            Err(error) => {
                self.live_error = Some(format!("repository overview reload failed: {error}"));
            }
        }
    }

    fn apply_repository_overview(&mut self, repository_overview: RepositoryOverview) {
        let intent = self.repository_review.replace(repository_overview);
        self.apply_repository_review_intent(intent);
    }

    fn apply_reloaded_changeset(&mut self, changeset: Changeset, preserve_scroll: bool) {
        let diff_position = self
            .selected_diff
            .capture_reload_position(self.selected_file());
        let reload = self
            .files_panel
            .review_reloaded(&self.changeset.files, &changeset.files);
        self.changeset = changeset;
        self.live_error = None;
        if matches!(self.overlay, Some(Overlay::Discard(_))) {
            self.overlay = None;
        }
        self.text_selection.clear();
        let selected_file_index = self.selected_file_index();
        self.selected_diff.review_reloaded(
            self.changeset.files.get(selected_file_index),
            reload.kept_file_selection,
            preserve_scroll,
            diff_position,
        );
        self.clear_render_caches();
        self.ensure_scroll_bounds();
    }

    fn clear_render_caches(&mut self) {
        self.selected_diff
            .clear_render_cache(self.changeset.files.len());
        self.text_selection.clear();
        self.pending_left_click = None;
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // A running command accepts only cancellation keys until it finishes.
        if matches!(self.overlay, Some(Overlay::CommandRunning { .. })) {
            self.handle_overlay_key(key);
            return Ok(true);
        }

        if is_ctrl_c(key) {
            return Ok(false);
        }

        if self.handle_global_ai_key(key) {
            return Ok(true);
        }

        self.text_selection.clear();

        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return Ok(true);
        }

        if self.selected_diff.search_prompt_open() {
            self.selected_diff.handle_search_prompt_key(key);
            self.ensure_scroll_bounds();
            return Ok(true);
        }

        if self.queue_copy_for_key(key) {
            return Ok(true);
        }

        if self.queue_custom_command_for_key(key) {
            return Ok(true);
        }

        match key.code {
            KeyCode::Tab => self.cycle_focus(VerticalDirection::Down),
            KeyCode::BackTab => self.cycle_focus(VerticalDirection::Up),
            KeyCode::Left if self.files_panel_visible => self.set_focus(FocusPane::Sidebar),
            KeyCode::Right | KeyCode::Enter => self.set_focus(FocusPane::Diff),
            KeyCode::Esc => self.selected_diff.clear_search_query(),

            KeyCode::PageDown => self.scroll_diff_page(VerticalDirection::Down),
            KeyCode::PageUp => self.scroll_diff_page(VerticalDirection::Up),
            KeyCode::Home => self.scroll_diff_to_top(),
            KeyCode::End => self.scroll_diff_to_bottom(),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_page(VerticalDirection::Down)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_page(VerticalDirection::Up)
            }
            _ => match self.keybinds.action_for(key) {
                Some(BuiltinAction::Quit) => return Ok(false),
                Some(BuiltinAction::Help) => self.toggle_help_overlay(),
                Some(BuiltinAction::ToggleFiles) => self.toggle_files_panel(),
                Some(BuiltinAction::Search) => {
                    self.focus = FocusPane::Diff;
                    self.selected_diff.open_search_prompt();
                }
                Some(BuiltinAction::MoveDown) => {
                    self.move_focused_pane_by(VerticalDirection::Down, 1)
                }
                Some(BuiltinAction::MoveUp) => self.move_focused_pane_by(VerticalDirection::Up, 1),
                Some(BuiltinAction::NextMatch) => {
                    let selected_file_index = self.selected_file_index();
                    self.selected_diff.jump_by(
                        VerticalDirection::Down,
                        selected_file_index,
                        self.changeset.files.get(selected_file_index),
                    );
                }
                Some(BuiltinAction::PrevMatch) => {
                    let selected_file_index = self.selected_file_index();
                    self.selected_diff.jump_by(
                        VerticalDirection::Up,
                        selected_file_index,
                        self.changeset.files.get(selected_file_index),
                    );
                }
                Some(BuiltinAction::Top) => self.scroll_diff_to_top(),
                Some(BuiltinAction::Bottom) => self.scroll_diff_to_bottom(),
                Some(BuiltinAction::ToggleStaging) => self.toggle_selected_staging(),
                Some(BuiltinAction::Discard) => self.request_selected_discard(),
                Some(BuiltinAction::Editor) => self.queue_selected_file_editor_request(),
                Some(BuiltinAction::ToggleReviewed) => self.toggle_selected_file_reviewed(),
                // AskAi, ExplainCode, UnpublishedSummary, CopyFocused, and CopyFileDiff are
                // dispatched earlier in this method, so they never reach here.
                Some(
                    BuiltinAction::AskAi
                    | BuiltinAction::ExplainCode
                    | BuiltinAction::UnpublishedSummary
                    | BuiltinAction::CopyFocused
                    | BuiltinAction::CopyFileDiff,
                ) => {}
                None => {}
            },
        }

        self.ensure_scroll_bounds();
        Ok(true)
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        // A running command blocks mouse input until it finishes or is cancelled.
        if matches!(self.overlay, Some(Overlay::CommandRunning { .. })) {
            return;
        }

        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            self.clear_live_notice();
        }

        if self.overlay.is_some() {
            self.handle_overlay_mouse(mouse);
            return;
        }

        let column = mouse.column;
        let row = mouse.row;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_left_down(column, row),
            MouseEventKind::Drag(MouseButton::Left) => self.handle_left_drag(column, row),
            MouseEventKind::Up(MouseButton::Left) => self.handle_left_up(column, row),
            MouseEventKind::ScrollDown => {
                self.handle_wheel_after_clearing_selection(column, row, VerticalDirection::Down);
            }
            MouseEventKind::ScrollUp => {
                self.handle_wheel_after_clearing_selection(column, row, VerticalDirection::Up);
            }
            MouseEventKind::Moved => self.handle_hover(column, row),
            _ => {}
        }

        self.ensure_scroll_bounds();
    }

    fn handle_selectable_text_mouse(
        &mut self,
        mouse: MouseEvent,
        mut scroll_by: impl FnMut(&mut Self, VerticalDirection),
    ) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.pending_left_click = None;
                self.text_selection.begin_drag(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.text_selection.update_drag(mouse.column, mouse.row);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.text_selection.finish_drag(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollDown => {
                self.text_selection.clear();
                scroll_by(self, VerticalDirection::Down);
            }
            MouseEventKind::ScrollUp => {
                self.text_selection.clear();
                scroll_by(self, VerticalDirection::Up);
            }
            _ => {}
        }
    }

    fn handle_wheel_after_clearing_selection(
        &mut self,
        column: u16,
        row: u16,
        direction: VerticalDirection,
    ) {
        self.text_selection.clear();
        self.handle_wheel(column, row, direction);
    }

    fn handle_left_down(&mut self, column: u16, row: u16) {
        self.selected_diff.cancel_scrollbar_drag();
        self.pending_left_click = None;

        if let Some(scroll) = self.selected_diff.start_scrollbar_drag(column, row) {
            self.set_focus(FocusPane::Diff);
            self.text_selection.clear();
            self.scroll_diff_to(scroll);
            return;
        }

        self.pending_left_click = self.pending_left_click_at(column, row);
        self.text_selection.begin_drag(column, row);
    }

    fn handle_left_up(&mut self, column: u16, row: u16) {
        if self.selected_diff.finish_scrollbar_drag() {
            return;
        }

        if let Some(target) = self.finish_pending_left_click(column, row) {
            self.apply_pending_left_click(target);
        }
    }

    fn finish_pending_left_click(&mut self, column: u16, row: u16) -> Option<PendingLeftClick> {
        if self.text_selection.finish_drag(column, row) {
            self.pending_left_click = None;
            return None;
        }

        self.pending_left_click.take()
    }

    fn apply_pending_left_click(&mut self, target: PendingLeftClick) {
        match target {
            PendingLeftClick::Sidebar { target } => {
                self.set_focus(FocusPane::Sidebar);
                if self
                    .files_panel
                    .activate(target, self.changeset.files.len())
                {
                    self.sync_selected_file();
                }
            }
            PendingLeftClick::Repository { target } => {
                let pane = target.pane();
                self.set_focus(pane.focus());
                let intent = self.repository_review.activate_target(target);
                self.apply_repository_review_intent(intent);
            }
            PendingLeftClick::Diff { hunk_index } => {
                self.set_focus(FocusPane::Diff);
                if !self.repository_review.details_active()
                    && let Some(index) = hunk_index
                {
                    let selected_file_index = self.selected_file_index();
                    self.selected_diff.select_hunk(
                        index,
                        selected_file_index,
                        self.changeset.files.get(selected_file_index),
                    );
                }
            }
        }
    }

    fn handle_left_drag(&mut self, column: u16, row: u16) {
        if self.drag_diff_scrollbar(row) {
            return;
        }

        self.pending_left_click = None;
        self.text_selection.update_drag(column, row);
    }

    fn pending_left_click_at(&self, column: u16, row: u16) -> Option<PendingLeftClick> {
        if let Some(target) = self.files_panel.target_at(column, row) {
            return Some(PendingLeftClick::Sidebar { target });
        }

        if let Some(target) = self.repository_review.target_at(column, row) {
            return Some(PendingLeftClick::Repository { target });
        }

        if self.is_diff_at(column, row) {
            return Some(PendingLeftClick::Diff {
                hunk_index: self.diff_hunk_index_at(column, row),
            });
        }

        None
    }

    fn drag_diff_scrollbar(&mut self, row: u16) -> bool {
        let Some(scroll) = self.selected_diff.drag_scrollbar(row) else {
            return false;
        };

        self.set_focus(FocusPane::Diff);
        self.scroll_diff_to(scroll);
        true
    }

    fn handle_hover(&mut self, column: u16, row: u16) {
        if let Some(focus) = self.pane_at(column, row) {
            self.set_focus(focus);
        }
    }

    fn handle_wheel(&mut self, column: u16, row: u16, direction: VerticalDirection) {
        let focus = self.pane_at(column, row).unwrap_or(FocusPane::Diff);
        self.set_focus(focus);
        self.move_focused_pane_by(direction, MOUSE_WHEEL_STEP);
    }

    fn pane_at(&self, column: u16, row: u16) -> Option<FocusPane> {
        if self.files_panel.contains(column, row) {
            return Some(FocusPane::Sidebar);
        }

        if let Some(pane) = self.repository_review.pane_at(column, row) {
            return Some(pane.focus());
        }

        self.is_diff_at(column, row).then_some(FocusPane::Diff)
    }

    fn is_diff_at(&self, column: u16, row: u16) -> bool {
        if self.repository_review.details_active() {
            self.repository_review.details_contains(column, row)
        } else {
            self.selected_diff.contains(column, row)
        }
    }

    fn diff_hunk_index_at(&self, column: u16, row: u16) -> Option<usize> {
        if self.repository_review.details_active() {
            return None;
        }
        let selected_file_index = self.selected_file_index();
        self.selected_diff.hunk_index_at(
            column,
            row,
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        )
    }

    fn cycle_focus(&mut self, direction: VerticalDirection) {
        if !self.files_panel_visible {
            self.set_focus(FocusPane::Diff);
            return;
        }

        let focus = match (self.focus, direction) {
            (FocusPane::Sidebar, VerticalDirection::Down)
            | (FocusPane::Commits, VerticalDirection::Up) => FocusPane::Branches,
            (FocusPane::Branches, VerticalDirection::Down)
            | (FocusPane::Diff, VerticalDirection::Up) => FocusPane::Commits,
            (FocusPane::Commits, VerticalDirection::Down)
            | (FocusPane::Sidebar, VerticalDirection::Up) => FocusPane::Diff,
            (FocusPane::Diff, VerticalDirection::Down)
            | (FocusPane::Branches, VerticalDirection::Up) => FocusPane::Sidebar,
        };
        self.set_focus(focus);
    }

    fn toggle_files_panel(&mut self) {
        self.files_panel_visible = !self.files_panel_visible;
        if !self.files_panel_visible {
            self.files_panel.clear_cursor();
        }
        let focus = if self.files_panel_visible {
            FocusPane::Sidebar
        } else {
            FocusPane::Diff
        };
        self.set_focus(focus);
    }

    fn handle_global_ai_key(&mut self, key: KeyEvent) -> bool {
        let Some(action) = self.global_ai_action_for_key(key) else {
            return false;
        };

        match action {
            GlobalAiAction::OpenHarnessPicker => self.open_harness_picker(),
            GlobalAiAction::OpenPrompt => self.open_ask_ai_prompt(),
            GlobalAiAction::ExplainCode => self.queue_explain_code_request(),
            GlobalAiAction::SummarizeUnpublished => {
                self.queue_unpublished_summary_request();
            }
        }
        self.text_selection.clear();
        self.ensure_scroll_bounds();
        true
    }

    fn global_ai_action_for_key(&self, key: KeyEvent) -> Option<GlobalAiAction> {
        if self.overlay.is_some() || self.selected_diff.search_prompt_open() {
            return None;
        }

        if key.code == KeyCode::F(2) {
            return Some(GlobalAiAction::OpenHarnessPicker);
        }

        match self.keybinds.action_for(key) {
            Some(BuiltinAction::AskAi) => Some(GlobalAiAction::OpenPrompt),
            Some(BuiltinAction::ExplainCode) => Some(GlobalAiAction::ExplainCode),
            Some(BuiltinAction::UnpublishedSummary) => Some(GlobalAiAction::SummarizeUnpublished),
            _ => None,
        }
    }

    fn focused_ask_ai_context(
        &self,
        missing_file_message: &'static str,
    ) -> Result<AskAiContext, &'static str> {
        let target = self
            .focused_review_target()
            .ask_ai(self.text_selection.selected_text(), missing_file_message)
            .map_err(|error| error.message())?;

        Ok(AskAiContext::focused(
            self.source.ask_ai_review_mode(),
            self.changeset.title.clone(),
            self.changeset.source_label.clone(),
            target.file,
            target.hunk_index,
            target.selected_text,
        ))
    }

    fn open_ask_ai_prompt(&mut self) {
        let context = match self.focused_ask_ai_context("no selected file to ask about") {
            Ok(context) => context,
            Err(message) => {
                self.live_error = Some(message.to_string());
                return;
            }
        };

        self.set_focus(FocusPane::Diff);
        self.live_error = None;
        self.overlay = Some(Overlay::AskAiPrompt(AskAiPromptState {
            context,
            input: String::new(),
        }));
    }

    fn submit_ask_ai_prompt(&mut self) {
        let Some(Overlay::AskAiPrompt(prompt)) = self.overlay.take() else {
            return;
        };

        let question = prompt.input.trim().to_string();
        if question.is_empty() {
            self.overlay = Some(Overlay::AskAiPrompt(prompt));
            return;
        }

        self.queue_ask_ai_request_effect(AskAiRequest::new(question, prompt.context));
    }

    fn queue_explain_code_request(&mut self) {
        let context = match self.focused_ask_ai_context("no selected file to explain") {
            Ok(context) => context,
            Err(message) => {
                self.live_error = Some(message.to_string());
                return;
            }
        };

        self.set_focus(FocusPane::Diff);
        self.live_error = None;
        self.queue_ask_ai_request_effect(AskAiRequest::explain_code(context));
    }

    fn queue_unpublished_summary_request(&mut self) {
        self.set_focus(FocusPane::Diff);
        self.live_error = None;
        self.queue_unpublished_summary_effect();
    }

    fn queue_copy_for_key(&mut self, key: KeyEvent) -> bool {
        match self.keybinds.action_for(key) {
            Some(BuiltinAction::CopyFocused) => self.queue_focused_target_copy(),
            Some(BuiltinAction::CopyFileDiff) => self.queue_selected_file_diff_copy(),
            _ => return false,
        }

        true
    }

    fn queue_focused_target_copy(&mut self) {
        if matches!(self.focus, FocusPane::Branches | FocusPane::Commits) {
            match self.focused_repository_copy() {
                Some((text, success_message)) => self.queue_clipboard_text(text, success_message),
                None if self.focus == FocusPane::Branches => {
                    self.set_copy_error("no selected branch to copy")
                }
                None => self.set_copy_error("no selected commit to copy"),
            }
            return;
        }

        let copy = match self.focused_review_target().copy() {
            Ok(FocusedCopyTarget::FilePath(target)) => Ok((
                target.file.display_path().to_string(),
                "copied selected file path",
            )),
            Ok(FocusedCopyTarget::HunkDiff(target)) => {
                patch::selected_hunk_patch(target.file, target.hunk_index)
                    .map(|text| (text, "copied selected hunk diff"))
                    .ok_or("no selected hunk diff to copy")
            }
            Err(error) => Err(error.message()),
        };

        self.apply_copy_result(copy);
    }

    fn focused_repository_copy(&self) -> Option<(String, &'static str)> {
        self.focus
            .repository_pane()
            .and_then(|pane| self.repository_review.copy(pane))
            .map(|copy| copy.into_parts())
    }

    fn queue_selected_file_diff_copy(&mut self) {
        if self.repository_review.details_active() {
            self.set_copy_error("no worktree file diff is active");
            return;
        }
        let copy = match self.focused_review_target().file_diff_copy() {
            Ok(target) => patch::file_patch(target.file)
                .map(|text| (text, "copied selected file diff"))
                .ok_or("no selected file diff to copy"),
            Err(error) => Err(error.message()),
        };

        self.apply_copy_result(copy);
    }

    fn apply_copy_result(&mut self, result: Result<(String, &'static str), &'static str>) {
        match result {
            Ok((text, success_message)) => self.queue_clipboard_text(text, success_message),
            Err(message) => self.set_copy_error(message),
        }
    }

    fn queue_ask_ai_answer_copy(&mut self) {
        let Some(answer) = self
            .ask_ai_output()
            .map(|output| output.result.stdout().to_string())
            .filter(|answer| !answer.trim().is_empty())
        else {
            self.set_copy_error("no Ask AI answer to copy");
            return;
        };

        self.queue_clipboard_text(answer, "copied Ask AI answer");
    }

    fn queue_clipboard_text(&mut self, text: String, success_message: &'static str) {
        self.live_error = None;
        self.clear_live_notice();
        self.queue_clipboard_effect(ClipboardRequest::new(text, success_message));
    }

    fn set_copy_error(&mut self, message: &'static str) {
        self.clear_clipboard_effect();
        self.clear_live_notice();
        self.live_error = Some(message.to_string());
    }

    fn queue_custom_command_for_key(&mut self, key: KeyEvent) -> bool {
        let Some(command) = self
            .custom_commands
            .iter()
            .find(|command| command.key().matches(key))
            .cloned()
        else {
            return false;
        };

        self.live_error = None;
        self.queue_custom_command_effect(command);
        true
    }

    fn move_focused_pane_by(&mut self, direction: VerticalDirection, amount: usize) {
        match self.focus {
            FocusPane::Sidebar => self.move_sidebar_cursor_by(direction, amount),
            FocusPane::Branches => {
                self.move_repository_selection_by(RepositoryPane::Branches, direction, amount)
            }
            FocusPane::Commits => {
                self.move_repository_selection_by(RepositoryPane::Commits, direction, amount)
            }
            FocusPane::Diff => self.scroll_diff_by(direction, amount),
        }
    }

    fn move_repository_selection_by(
        &mut self,
        pane: RepositoryPane,
        direction: VerticalDirection,
        amount: usize,
    ) {
        let intent = self
            .repository_review
            .move_selection(pane, direction, amount);
        self.apply_repository_review_intent(intent);
    }

    fn move_sidebar_cursor_by(&mut self, direction: VerticalDirection, amount: usize) {
        if self
            .files_panel
            .move_cursor(direction, amount, self.changeset.files.len())
        {
            self.sync_selected_file();
        }
    }

    fn sync_selected_file(&mut self) {
        let selected_file_index = self.selected_file_index();
        self.selected_diff
            .select_file(self.changeset.files.get(selected_file_index));
    }

    fn scroll_diff_page(&mut self, direction: VerticalDirection) {
        if self.repository_review.details_active() {
            self.repository_review.scroll_details_page(direction);
            return;
        }
        let selected_file_index = self.selected_file_index();
        self.selected_diff.scroll_page(
            direction,
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        );
    }

    fn scroll_diff_by(&mut self, direction: VerticalDirection, amount: usize) {
        if self.repository_review.details_active() {
            self.repository_review.scroll_details_by(direction, amount);
            return;
        }
        let selected_file_index = self.selected_file_index();
        self.selected_diff.scroll_by(
            direction,
            amount,
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        );
    }

    fn scroll_diff_to(&mut self, scroll: usize) {
        if self.repository_review.details_active() {
            return;
        }
        let selected_file_index = self.selected_file_index();
        self.selected_diff.scroll_to(
            scroll,
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        );
    }

    fn scroll_diff_to_top(&mut self) {
        if self.repository_review.details_active() {
            self.repository_review.scroll_details_to_top();
            return;
        }
        let selected_file_index = self.selected_file_index();
        self.selected_diff.scroll_to_top(
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        );
    }

    fn scroll_diff_to_bottom(&mut self) {
        if self.repository_review.details_active() {
            self.repository_review.scroll_details_to_bottom();
            return;
        }
        let selected_file_index = self.selected_file_index();
        self.selected_diff
            .scroll_to_bottom(self.changeset.files.get(selected_file_index));
    }

    fn toggle_selected_staging(&mut self) {
        if !self.can_stage() {
            return;
        }

        let request = if let Some(request) = self.active_folder_staging_request() {
            request
        } else {
            match self.focused_review_target().staging() {
                Ok(Some(FocusedMutationTarget::File(target))) => StagingRequest::File {
                    path: target.file.display_path().to_string(),
                },
                Ok(Some(FocusedMutationTarget::Hunk(target))) => StagingRequest::Hunk {
                    file: target.file.clone(),
                    hunk_index: target.hunk_index,
                },
                Ok(None) => return,
                Err(error) => {
                    self.live_error = Some(error.message().to_string());
                    return;
                }
            }
        };

        self.apply_worktree_mutation(request.into_mutation());
    }

    fn toggle_selected_file_reviewed(&mut self) {
        if self.repository_review.details_active() {
            return;
        }
        self.files_panel
            .toggle_selected_reviewed(&self.changeset.files);
    }

    #[cfg(test)]
    pub(super) fn is_selected_file_reviewed(&self) -> bool {
        self.files_panel
            .is_selected_file_reviewed(&self.changeset.files)
    }

    fn request_selected_discard(&mut self) {
        if !self.can_discard() {
            return;
        }

        let Some(target) = self
            .active_folder_discard_target()
            .or_else(|| self.focused_discard_target())
        else {
            return;
        };

        self.live_error = None;
        self.overlay = Some(Overlay::Discard(DiscardConfirmation { target }));
    }

    fn active_folder_staging_request(&mut self) -> Option<StagingRequest> {
        if self.focus != FocusPane::Sidebar || !self.files_panel_visible {
            return None;
        }

        let folder = self.files_panel.focused_folder()?;
        let file_paths = folder
            .file_indices
            .iter()
            .filter_map(|index| self.changeset.files.get(*index))
            .map(|file| file.display_path().to_string())
            .collect::<Vec<_>>();
        if file_paths.is_empty() {
            self.live_error = Some(format!("no changed files in folder {}", folder.path));
            return None;
        }

        let all_staged = folder
            .file_indices
            .iter()
            .filter_map(|index| self.changeset.files.get(*index))
            .all(|file| file.stage == FileStage::Staged);

        Some(StagingRequest::Folder {
            path: folder.path,
            file_paths,
            action: if all_staged {
                FolderStagingAction::Unstage
            } else {
                FolderStagingAction::Stage
            },
        })
    }

    fn active_folder_discard_target(&mut self) -> Option<DiscardTarget> {
        if self.focus != FocusPane::Sidebar || !self.files_panel_visible {
            return None;
        }

        let folder = self.files_panel.focused_folder()?;
        let file_paths = folder
            .file_indices
            .iter()
            .filter_map(|index| self.changeset.files.get(*index))
            .filter(|file| matches!(file.stage, FileStage::Unstaged | FileStage::Mixed))
            .map(|file| file.display_path().to_string())
            .collect::<Vec<_>>();
        if file_paths.is_empty() {
            self.live_error = Some(format!(
                "no unstaged worktree changes in folder {}",
                folder.path
            ));
            return None;
        }

        Some(DiscardTarget::Folder {
            path: folder.path,
            file_paths,
        })
    }

    fn current_folder_paths(&self, folder_path: &str) -> Vec<String> {
        self.files_panel
            .folder_file_indices(folder_path)
            .unwrap_or_default()
            .iter()
            .filter_map(|index| self.changeset.files.get(*index))
            .map(|file| file.display_path().to_string())
            .collect()
    }

    fn confirmed_folder_paths(
        &mut self,
        path: &str,
        file_paths: Vec<String>,
    ) -> Option<Vec<String>> {
        if file_paths.is_empty() {
            self.live_error = Some("discard failed: selected folder has no files".to_string());
            return None;
        }

        let current_paths = self.current_folder_paths(path);
        let all_still_present = file_paths
            .iter()
            .all(|file_path| current_paths.iter().any(|current| current == file_path));
        if !all_still_present {
            self.live_error =
                Some("discard failed: selected folder changed before confirmation".to_string());
            return None;
        }

        Some(file_paths)
    }

    fn focused_discard_target(&mut self) -> Option<DiscardTarget> {
        match self.focused_review_target().discard() {
            Ok(Some(FocusedMutationTarget::File(target))) => Some(DiscardTarget::File {
                file_index: target.file_index,
                path: target.file.display_path().to_string(),
            }),
            Ok(Some(FocusedMutationTarget::Hunk(target))) => Some(DiscardTarget::Hunk {
                file_index: target.file_index,
                hunk_index: target.hunk_index,
                path: target.file.display_path().to_string(),
            }),
            Ok(None) => None,
            Err(error) => {
                self.live_error = Some(error.message().to_string());
                None
            }
        }
    }

    fn execute_pending_discard(&mut self) {
        let Some(Overlay::Discard(confirmation)) = self.overlay.take() else {
            return;
        };

        let mutation = match confirmation.target {
            DiscardTarget::File { file_index, path } => {
                if self.confirmed_file(file_index, &path).is_none() {
                    return;
                }
                WorktreeMutation::DiscardFile { path }
            }
            DiscardTarget::Folder { path, file_paths } => {
                let Some(file_paths) = self.confirmed_folder_paths(&path, file_paths) else {
                    return;
                };
                WorktreeMutation::DiscardFiles { paths: file_paths }
            }
            DiscardTarget::Hunk {
                file_index,
                hunk_index,
                path,
            } => {
                let Some(file) = self.confirmed_file(file_index, &path) else {
                    return;
                };
                WorktreeMutation::DiscardHunk { file, hunk_index }
            }
        };

        self.apply_worktree_mutation(mutation);
    }

    fn apply_worktree_mutation(&mut self, mutation: WorktreeMutation) {
        let Some(mutations) = self.source.worktree_mutations() else {
            return;
        };
        let preserve_scroll = mutation.preserve_scroll();
        let failure_context = mutation.failure_context();

        match mutations.apply(mutation) {
            Ok(reloaded_changeset) => {
                self.apply_reloaded_changeset(reloaded_changeset, preserve_scroll)
            }
            Err(error) => self.live_error = Some(format!("{failure_context}: {error}")),
        }
    }

    fn confirmed_file(&mut self, file_index: usize, path: &str) -> Option<DiffFile> {
        let Some(file) = self.changeset.files.get(file_index).cloned() else {
            self.live_error = Some("discard failed: selected file no longer exists".to_string());
            return None;
        };

        if file.display_path() != path {
            self.live_error =
                Some("discard failed: selected file changed before confirmation".to_string());
            return None;
        }

        Some(file)
    }

    fn queue_selected_file_editor_request(&mut self) {
        if self.repository_review.details_active() {
            self.live_error = Some("no worktree file is active".to_string());
            return;
        }
        let request = match self.focused_review_target().editor() {
            Ok(target) => self
                .source
                .editor_request(target.file, self.editor_line(target.file)),
            Err(error) => {
                self.live_error = Some(error.message().to_string());
                return;
            }
        };

        match request {
            Ok(request) => {
                self.live_error = None;
                self.queue_editor_effect(request);
            }
            Err(error) => self.live_error = Some(format!("edit failed: {error}")),
        }
    }

    fn editor_line(&self, file: &DiffFile) -> Option<u32> {
        if self.focus == FocusPane::Diff {
            let selected_file_index = self.selected_file_index();
            return self
                .selected_diff
                .editor_line(selected_file_index, file)
                .or_else(|| file.first_changed_line());
        }

        file.first_changed_line()
    }
}

fn saturating_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn pane_text_area(area: Rect, content_width: usize, visible_height: usize) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: saturating_u16(content_width),
        height: saturating_u16(visible_height),
    }
}

#[cfg(test)]
fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[cfg(test)]
mod tests;
