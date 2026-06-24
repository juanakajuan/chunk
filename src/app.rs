//! Terminal application session state.
//!
//! `App` owns selection, focus, scroll state, and live reload errors. Review
//! source behavior lives in `review_source`; terminal and watch orchestration
//! live in `runtime`; rendered row preparation lives here while `ui` draws
//! Ratatui widgets.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::ask_ai::{AskAiContext, AskAiRequest};
use crate::config::AppConfig;
use crate::custom_command::CustomCommandBinding;
use crate::editor::EditorRequest;
use crate::keybind::{BuiltinAction, KeybindMap};
use crate::model::{Changeset, DiffFile, DiffHunk, FileStage};
use crate::patch;
use crate::review_source::{LoadedReview, ReviewSource, WorktreeMutation};
use crate::rows::{self, SidebarRowCountsInput, SidebarRowTarget, SidebarRowsInput};
use crate::scroll_text::VerticalDirection;
use crate::selection::TextSelection;
use crate::theme::{Theme, ThemeName};
use crate::viewport::{DiffScrollbar, DiffScrollbarDrag, RenderedViewport, ViewportScrollInput};

mod diff_frame;
mod diff_pane;
mod focused_review_target;
mod keys;
mod overlay;
mod reload;
mod search;

pub(crate) use keys::accepts_text_input;

use diff_pane::DiffPaneState;
use focused_review_target::{FocusedCopyTarget, FocusedMutationTarget, FocusedReviewTarget};
use keys::is_ctrl_c;
use overlay::{AskAiPromptState, DiscardConfirmation, DiscardTarget, Overlay};

const MOUSE_WHEEL_STEP: usize = 3;
const HELP_OVERLAY_SCROLL_PAGE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Sidebar,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingLeftClick {
    Sidebar { target: SidebarRowTarget },
    Diff { hunk_index: Option<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyAction {
    FocusedTarget,
    SelectedFileDiff,
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
    OpenEditor(EditorRequest),
    RunCustomCommand(CustomCommandBinding),
    CancelCustomCommand,
    RunAskAi(AskAiRequest),
    RunUnpublishedSummary,
    CancelAskAi,
    CopyToClipboard(ClipboardRequest),
}

#[derive(Debug)]
pub(crate) struct App {
    /// Review source behavior for this session.
    source: ReviewSource,
    /// Current diff data being reviewed.
    changeset: Changeset,
    /// Last live reload/watch error, rendered above the diff when present.
    live_error: Option<String>,
    /// Last successful background/status event, rendered above the diff.
    live_notice: Option<String>,
    /// Index into `changeset.files`.
    selected_file_index: usize,
    /// Diff-pane scroll, hunk selection, search, and render-derived navigation state.
    diff_pane: DiffPaneState,
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
    /// Deferred runtime effects requested by application behavior.
    effects: VecDeque<AppEffect>,
    /// First file index considered for sidebar rendering.
    sidebar_scroll: usize,
    /// Collapsed tree directory paths in the current session.
    collapsed_tree_dirs: HashSet<String>,
    /// Active Files-panel tree row, if it differs from the selected file fallback.
    sidebar_cursor_target: Option<SidebarRowTarget>,
    /// Rendered viewport geometry, row mapping, and render caches.
    viewport: RenderedViewport,
    /// Visible text rows and active drag-to-copy selection.
    text_selection: TextSelection,
    /// Deferred plain click target, cancelled if the press becomes a drag.
    pending_left_click: Option<PendingLeftClick>,
    /// Active mouse drag against the rendered diff scrollbar.
    diff_scrollbar_drag: Option<DiffScrollbarDrag>,
    /// Display paths marked reviewed in this session. Keyed by `display_path`
    /// so review state survives reloads as long as file identity stays stable.
    reviewed_files: HashSet<String>,
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
        let LoadedReview { source, changeset } = review;
        let file_count = changeset.files.len();
        let diff_pane = DiffPaneState::new(&changeset);
        Self {
            source,
            changeset,
            live_error: None,
            live_notice: None,
            selected_file_index: 0,
            diff_pane,
            focus: FocusPane::Sidebar,
            files_panel_visible: true,
            overlay: None,
            custom_commands: config.commands,
            keybinds: config.keybinds,
            theme: config.theme,
            effects: VecDeque::new(),
            sidebar_scroll: 0,
            collapsed_tree_dirs: HashSet::new(),
            sidebar_cursor_target: None,
            viewport: RenderedViewport::new(file_count),
            text_selection: TextSelection::default(),
            pending_left_click: None,
            diff_scrollbar_drag: None,
            reviewed_files: HashSet::new(),
        }
    }

    pub(crate) fn begin_render_frame(&mut self) {
        self.viewport.begin_frame();
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
        self.viewport.begin_sidebar(area, visible_height);
        self.ensure_scroll_bounds();

        let row_counts_input = SidebarRowCountsInput {
            files: &self.changeset.files,
            collapsed_dirs: &self.collapsed_tree_dirs,
            content_width,
            theme,
        };
        let row_counts = rows::sidebar_row_counts(row_counts_input);

        let rendered_rows = rows::sidebar_rows(SidebarRowsInput {
            files: &self.changeset.files,
            empty_message: self.empty_sidebar_message(),
            collapsed_dirs: &self.collapsed_tree_dirs,
            selected_file_index: self.selected_file_index,
            sidebar_scroll: self.sidebar_scroll,
            row_counts: &row_counts,
            content_width,
            visible_height,
            theme,
            reviewed_files: &self.reviewed_files,
            active_target: self.sidebar_cursor_target.as_ref(),
        });
        self.sidebar_scroll = rendered_rows.sidebar_scroll;
        self.viewport.begin_sidebar_rows();
        for record in rendered_rows.row_records {
            self.viewport
                .record_sidebar_rows(record.target, record.row_count);
        }

        self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            rendered_rows.lines,
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
            &self.custom_commands,
            self.keybinds,
            content_width,
            theme,
        )
    }

    fn can_stage(&self) -> bool {
        self.source.worktree_mutations().is_some()
    }

    fn can_discard(&self) -> bool {
        self.source.worktree_mutations().is_some()
    }

    fn stage_keybind_hint(&self) -> Option<&'static str> {
        if !self.can_stage() {
            return None;
        }

        self.focused_review_target().stage_keybind_hint()
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
        self.changeset.files.get(self.selected_file_index)
    }

    fn selected_hunk(&self) -> Option<&DiffHunk> {
        self.diff_pane.selected_hunk(self.selected_file())
    }

    fn focused_review_target(&self) -> FocusedReviewTarget<'_> {
        FocusedReviewTarget::new(
            &self.changeset.files,
            self.selected_file_index,
            self.diff_pane.selected_hunk_index(),
            self.focus,
            self.files_panel_visible,
        )
    }

    fn ensure_scroll_bounds(&mut self) {
        let selected_file = self.changeset.files.get(self.selected_file_index);
        let input = ViewportScrollInput {
            diff_scroll: self.diff_pane.scroll(),
            sidebar_scroll: self.sidebar_scroll,
            selected_file_index: self.selected_file_index,
            file_count: self.changeset.files.len(),
            selected_file_id: selected_file.map(|file| file.id.as_str()),
            selected_file_line_count: selected_file.map_or(0, DiffFile::line_count),
        };

        self.sidebar_scroll = self
            .diff_pane
            .ensure_bounds(selected_file, &self.viewport, input);
    }

    pub(crate) fn set_live_error(&mut self, error: String) {
        self.live_notice = None;
        self.live_error = Some(error);
    }

    pub(crate) fn set_live_notice(&mut self, notice: String) {
        self.live_error = None;
        self.live_notice = Some(notice);
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

    fn queue_custom_command_effect(&mut self, command: CustomCommandBinding) {
        self.clear_effects(|effect| matches!(effect, AppEffect::RunCustomCommand(_)));
        self.effects.push_back(AppEffect::RunCustomCommand(command));
    }

    fn queue_custom_command_cancel_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::CancelCustomCommand));
        self.effects.push_back(AppEffect::CancelCustomCommand);
    }

    fn queue_ask_ai_request_effect(&mut self, request: AskAiRequest) {
        self.clear_effects(|effect| matches!(effect, AppEffect::RunAskAi(_)));
        self.effects.push_back(AppEffect::RunAskAi(request));
    }

    fn queue_unpublished_summary_effect(&mut self) {
        self.clear_effects(|effect| matches!(effect, AppEffect::RunUnpublishedSummary));
        self.effects.push_back(AppEffect::RunUnpublishedSummary);
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
            Ok(changeset) => self.apply_reloaded_changeset(changeset, preserve_scroll),
            Err(error) => self.live_error = Some(format!("reload failed: {error}")),
        }
    }

    fn apply_reloaded_changeset(&mut self, changeset: Changeset, preserve_scroll: bool) {
        reload::apply_changeset(self, changeset, preserve_scroll);
    }

    fn clear_render_caches(&mut self) {
        self.viewport
            .clear_render_caches(self.changeset.files.len());
        self.text_selection.clear();
        self.pending_left_click = None;
        self.diff_scrollbar_drag = None;
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

        if self.should_open_ask_ai_prompt(key) {
            self.open_ask_ai_prompt();
            self.text_selection.clear();
            self.ensure_scroll_bounds();
            return Ok(true);
        }

        if self.should_queue_explain_code_request(key) {
            self.queue_explain_code_request();
            self.text_selection.clear();
            self.ensure_scroll_bounds();
            return Ok(true);
        }

        if self.should_queue_unpublished_summary_request(key) {
            self.queue_unpublished_summary_request();
            self.text_selection.clear();
            self.ensure_scroll_bounds();
            return Ok(true);
        }

        self.text_selection.clear();

        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return Ok(true);
        }

        if self.search_prompt_open() {
            self.handle_search_prompt_key(key);
            return Ok(true);
        }

        if self.queue_copy_for_key(key) {
            return Ok(true);
        }

        if self.queue_custom_command_for_key(key) {
            return Ok(true);
        }

        match key.code {
            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Left if self.files_panel_visible => self.focus = FocusPane::Sidebar,
            KeyCode::Right | KeyCode::Enter => self.focus = FocusPane::Diff,
            KeyCode::Esc => self.clear_search_query(),

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
                Some(BuiltinAction::Search) => self.open_search_prompt(),
                Some(BuiltinAction::MoveDown) => self.move_by(VerticalDirection::Down),
                Some(BuiltinAction::MoveUp) => self.move_by(VerticalDirection::Up),
                Some(BuiltinAction::NextMatch) => self.jump_by(VerticalDirection::Down),
                Some(BuiltinAction::PrevMatch) => self.jump_by(VerticalDirection::Up),
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
        self.diff_scrollbar_drag = None;
        self.pending_left_click = None;

        if let Some((drag, scroll)) = self.diff_scrollbar_drag_at(column, row) {
            self.focus = FocusPane::Diff;
            self.text_selection.clear();
            self.diff_scrollbar_drag = Some(drag);
            self.scroll_diff_to(scroll);
            return;
        }

        self.pending_left_click = self.pending_left_click_at(column, row);
        self.text_selection.begin_drag(column, row);
    }

    fn handle_left_up(&mut self, column: u16, row: u16) {
        if self.diff_scrollbar_drag.take().is_some() {
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
                self.focus = FocusPane::Sidebar;
                match target {
                    SidebarRowTarget::File(index) => self.select_file(index),
                    SidebarRowTarget::Folder(path) => self.toggle_tree_folder(&path),
                }
            }
            PendingLeftClick::Diff { hunk_index } => {
                self.focus = FocusPane::Diff;
                if let Some(index) = hunk_index {
                    self.diff_pane.set_selected_hunk_index(Some(index));
                    self.diff_pane.center_selected_hunk(
                        &self.viewport,
                        self.selected_file_index,
                        self.changeset.files.get(self.selected_file_index),
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
        if let Some(target) = self.sidebar_target_at(column, row) {
            return Some(PendingLeftClick::Sidebar { target });
        }

        if self.is_diff_at(column, row) {
            return Some(PendingLeftClick::Diff {
                hunk_index: self.diff_hunk_index_at(column, row),
            });
        }

        None
    }

    fn drag_diff_scrollbar(&mut self, row: u16) -> bool {
        let Some(drag) = self.diff_scrollbar_drag else {
            return false;
        };
        let Some(scrollbar) = self.viewport.diff_scrollbar() else {
            self.diff_scrollbar_drag = None;
            return true;
        };

        self.focus = FocusPane::Diff;
        self.scroll_diff_to(scrollbar.scroll_for_drag(row, drag));
        true
    }

    fn handle_hover(&mut self, column: u16, row: u16) {
        if let Some(focus) = self.pane_at(column, row) {
            self.focus = focus;
        }
    }

    fn handle_wheel(&mut self, column: u16, row: u16, direction: VerticalDirection) {
        let focus = self.pane_at(column, row).unwrap_or(FocusPane::Diff);
        self.focus = focus;

        match focus {
            FocusPane::Sidebar => self.move_sidebar_cursor_by(direction, MOUSE_WHEEL_STEP),
            FocusPane::Diff => self.scroll_diff_by(direction, MOUSE_WHEEL_STEP),
        }
    }

    fn pane_at(&self, column: u16, row: u16) -> Option<FocusPane> {
        if self.is_sidebar_at(column, row) {
            return Some(FocusPane::Sidebar);
        }

        self.is_diff_at(column, row).then_some(FocusPane::Diff)
    }

    fn sidebar_target_at(&self, column: u16, row: u16) -> Option<SidebarRowTarget> {
        self.viewport.sidebar_target_at(column, row)
    }

    fn is_sidebar_at(&self, column: u16, row: u16) -> bool {
        self.viewport.is_sidebar_at(column, row)
    }

    fn is_diff_at(&self, column: u16, row: u16) -> bool {
        self.viewport.is_diff_at(column, row)
    }

    fn diff_hunk_index_at(&self, column: u16, row: u16) -> Option<usize> {
        let visible_row = self.viewport.diff_row_at(column, row)?;
        let diff_row = visible_row.checked_sub(self.viewport.diff_status_rows())?;

        self.diff_pane.hunk_index_at_rendered_row(
            &self.viewport,
            self.selected_file_index,
            self.changeset.files.get(self.selected_file_index),
            self.diff_pane.scroll().saturating_add(diff_row),
        )
    }

    fn diff_scrollbar_drag_at(&self, column: u16, row: u16) -> Option<(DiffScrollbarDrag, usize)> {
        let scrollbar = self.viewport.diff_scrollbar()?;
        let drag = scrollbar.drag_at(column, row)?;
        Some((drag, scrollbar.scroll_for_drag(row, drag)))
    }

    fn toggle_focus(&mut self) {
        if !self.files_panel_visible {
            self.focus = FocusPane::Diff;
            return;
        }

        self.focus = match self.focus {
            FocusPane::Sidebar => FocusPane::Diff,
            FocusPane::Diff => FocusPane::Sidebar,
        };
    }

    fn toggle_files_panel(&mut self) {
        self.files_panel_visible = !self.files_panel_visible;
        if !self.files_panel_visible {
            self.sidebar_cursor_target = None;
        }
        self.focus = if self.files_panel_visible {
            FocusPane::Sidebar
        } else {
            FocusPane::Diff
        };
    }

    fn toggle_tree_folder(&mut self, path: &str) {
        if !self.collapsed_tree_dirs.insert(path.to_string()) {
            self.collapsed_tree_dirs.remove(path);
        }
    }

    fn should_open_ask_ai_prompt(&self, key: KeyEvent) -> bool {
        self.overlay.is_none()
            && !self.search_prompt_open()
            && self.keybinds.action_for(key) == Some(BuiltinAction::AskAi)
    }

    fn should_queue_explain_code_request(&self, key: KeyEvent) -> bool {
        self.overlay.is_none()
            && !self.search_prompt_open()
            && self.keybinds.action_for(key) == Some(BuiltinAction::ExplainCode)
    }

    fn should_queue_unpublished_summary_request(&self, key: KeyEvent) -> bool {
        self.overlay.is_none()
            && !self.search_prompt_open()
            && self.keybinds.action_for(key) == Some(BuiltinAction::UnpublishedSummary)
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

        self.focus = FocusPane::Diff;
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

        self.focus = FocusPane::Diff;
        self.live_error = None;
        self.queue_ask_ai_request_effect(AskAiRequest::explain_code(context));
    }

    fn queue_unpublished_summary_request(&mut self) {
        self.focus = FocusPane::Diff;
        self.live_error = None;
        self.queue_unpublished_summary_effect();
    }

    fn queue_copy_for_key(&mut self, key: KeyEvent) -> bool {
        match self.keybinds.action_for(key) {
            Some(BuiltinAction::CopyFocused) => self.queue_copy_action(CopyAction::FocusedTarget),
            Some(BuiltinAction::CopyFileDiff) => {
                self.queue_copy_action(CopyAction::SelectedFileDiff)
            }
            _ => return false,
        }

        true
    }

    fn queue_copy_action(&mut self, action: CopyAction) {
        match action {
            CopyAction::FocusedTarget => self.queue_focused_target_copy(),
            CopyAction::SelectedFileDiff => self.queue_selected_file_diff_copy(),
        }
    }

    fn queue_focused_target_copy(&mut self) {
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

        match copy {
            Ok((text, success_message)) => self.queue_clipboard_text(text, success_message),
            Err(message) => self.set_copy_error(message),
        }
    }

    fn queue_selected_file_diff_copy(&mut self) {
        let copy = match self.focused_review_target().file_diff_copy() {
            Ok(target) => patch::file_patch(target.file)
                .map(|text| (text, "copied selected file diff"))
                .ok_or("no selected file diff to copy"),
            Err(error) => Err(error.message()),
        };

        match copy {
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
        self.live_notice = None;
        self.queue_clipboard_effect(ClipboardRequest::new(text, success_message));
    }

    fn set_copy_error(&mut self, message: &'static str) {
        self.clear_clipboard_effect();
        self.live_notice = None;
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

    fn move_by(&mut self, direction: VerticalDirection) {
        match self.focus {
            FocusPane::Sidebar => self.move_sidebar_cursor_by(direction, 1),
            FocusPane::Diff => self.scroll_diff_by(direction, 1),
        }
    }

    fn move_sidebar_cursor_by(&mut self, direction: VerticalDirection, amount: usize) {
        let visible_targets =
            rows::visible_sidebar_targets(&self.changeset.files, &self.collapsed_tree_dirs);
        if visible_targets.is_empty() {
            return;
        }

        let current_target = self.current_sidebar_target();
        let target_position = match current_target.as_ref().and_then(|target| {
            visible_targets
                .iter()
                .position(|candidate| candidate == target)
        }) {
            Some(current_position) => {
                direction.shift_clamped(current_position, amount, visible_targets.len() - 1)
            }
            None => fallback_sidebar_target_position(
                &visible_targets,
                self.selected_file_index,
                direction,
            ),
        };
        let target = visible_targets[target_position].clone();
        self.apply_sidebar_cursor_target(target);
    }

    fn current_sidebar_target(&self) -> Option<SidebarRowTarget> {
        self.sidebar_cursor_target.clone().or_else(|| {
            (!self.changeset.files.is_empty())
                .then_some(SidebarRowTarget::File(self.selected_file_index))
        })
    }

    fn apply_sidebar_cursor_target(&mut self, target: SidebarRowTarget) {
        self.sidebar_cursor_target = Some(target.clone());
        if let SidebarRowTarget::File(index) = target {
            self.select_file(index);
        }
    }

    fn select_file(&mut self, index: usize) {
        if self.changeset.files.is_empty() {
            return;
        }

        self.selected_file_index = index.min(self.changeset.files.len() - 1);
        self.sidebar_cursor_target = Some(SidebarRowTarget::File(self.selected_file_index));
        self.diff_pane
            .select_file(self.changeset.files.get(self.selected_file_index));
    }

    fn scroll_diff_page(&mut self, direction: VerticalDirection) {
        self.diff_pane.scroll_page(
            direction,
            &self.viewport,
            self.selected_file_index,
            self.changeset.files.get(self.selected_file_index),
        );
    }

    fn scroll_diff_by(&mut self, direction: VerticalDirection, amount: usize) {
        self.diff_pane.scroll_by(
            direction,
            amount,
            &self.viewport,
            self.selected_file_index,
            self.changeset.files.get(self.selected_file_index),
        );
    }

    fn scroll_diff_to(&mut self, scroll: usize) {
        self.diff_pane.scroll_to(
            scroll,
            &self.viewport,
            self.selected_file_index,
            self.changeset.files.get(self.selected_file_index),
        );
    }

    fn scroll_diff_to_top(&mut self) {
        self.diff_pane.scroll_to_top(
            &self.viewport,
            self.selected_file_index,
            self.changeset.files.get(self.selected_file_index),
        );
    }

    fn scroll_diff_to_bottom(&mut self) {
        self.diff_pane
            .scroll_to_bottom(self.changeset.files.get(self.selected_file_index));
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
        let Some(file) = self.selected_file() else {
            return;
        };

        let path = file.display_path().to_string();
        if !self.reviewed_files.insert(path.clone()) {
            self.reviewed_files.remove(&path);
        }
    }

    #[cfg(test)]
    pub(super) fn is_selected_file_reviewed(&self) -> bool {
        self.selected_file()
            .map(|file| self.reviewed_files.contains(file.display_path()))
            .unwrap_or(false)
    }

    fn request_selected_discard(&mut self) {
        if !self.can_discard() {
            return;
        }

        let target = if let Some(target) = self.active_folder_discard_target() {
            target
        } else {
            let Some(target) = self.focused_discard_target() else {
                return;
            };
            target
        };

        self.live_error = None;
        self.overlay = Some(Overlay::Discard(DiscardConfirmation { target }));
    }

    fn active_folder_staging_request(&mut self) -> Option<StagingRequest> {
        if self.focus != FocusPane::Sidebar || !self.files_panel_visible {
            return None;
        }

        let Some(SidebarRowTarget::Folder(path)) = self.sidebar_cursor_target.as_ref() else {
            return None;
        };
        let file_paths = self.current_folder_paths(path);
        if file_paths.is_empty() {
            self.live_error = Some(format!("no changed files in folder {path}"));
            return None;
        }

        Some(StagingRequest::Folder {
            path: path.clone(),
            file_paths,
            action: self.folder_staging_action(path),
        })
    }

    fn folder_staging_action(&self, folder_path: &str) -> FolderStagingAction {
        if self
            .changeset
            .files
            .iter()
            .filter(|file| folder_contains_path(folder_path, file.display_path()))
            .all(|file| file.stage == FileStage::Staged)
        {
            FolderStagingAction::Unstage
        } else {
            FolderStagingAction::Stage
        }
    }

    fn active_folder_discard_target(&mut self) -> Option<DiscardTarget> {
        if self.focus != FocusPane::Sidebar || !self.files_panel_visible {
            return None;
        }

        let Some(SidebarRowTarget::Folder(path)) = self.sidebar_cursor_target.as_ref() else {
            return None;
        };
        let file_paths = self.folder_discard_paths(path);
        if file_paths.is_empty() {
            self.live_error = Some(format!("no unstaged worktree changes in folder {path}"));
            return None;
        }

        Some(DiscardTarget::Folder {
            path: path.clone(),
            file_paths,
        })
    }

    fn folder_discard_paths(&self, folder_path: &str) -> Vec<String> {
        self.changeset
            .files
            .iter()
            .filter(|file| {
                folder_contains_path(folder_path, file.display_path())
                    && matches!(file.stage, FileStage::Unstaged | FileStage::Mixed)
            })
            .map(|file| file.display_path().to_string())
            .collect()
    }

    fn current_folder_paths(&self, folder_path: &str) -> Vec<String> {
        self.changeset
            .files
            .iter()
            .filter(|file| folder_contains_path(folder_path, file.display_path()))
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
        self.clear_effects(|effect| matches!(effect, AppEffect::OpenEditor(_)));
        let request = match self.focused_review_target().editor() {
            Ok(target) => self.source.editor_request(target.file),
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

fn fallback_sidebar_target_position(
    visible_targets: &[SidebarRowTarget],
    selected_file_index: usize,
    direction: VerticalDirection,
) -> usize {
    match direction {
        VerticalDirection::Down => visible_targets
            .iter()
            .position(|target| {
                matches!(target, SidebarRowTarget::File(index) if *index > selected_file_index)
            })
            .unwrap_or(visible_targets.len() - 1),
        VerticalDirection::Up => visible_targets
            .iter()
            .rposition(|target| {
                matches!(target, SidebarRowTarget::File(index) if *index < selected_file_index)
            })
            .unwrap_or(0),
    }
}

fn folder_contains_path(folder_path: &str, path: &str) -> bool {
    path.strip_prefix(folder_path)
        .is_some_and(|suffix| suffix.starts_with('/'))
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
