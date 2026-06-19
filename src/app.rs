//! Terminal application session state.
//!
//! `App` owns selection, focus, scroll state, and live reload errors. Review
//! source behavior lives in `review_source`; terminal and watch orchestration
//! live in `runtime`; rendered row preparation lives here while `ui` draws
//! Ratatui widgets.

use std::collections::HashSet;
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
use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::patch;
use crate::review_source::{LoadedReview, ReviewSource};
use crate::rows::{self, SidebarRowsInput};
use crate::scroll_text::VerticalDirection;
use crate::search::Search;
use crate::selection::TextSelection;
use crate::theme::{Theme, ThemeName};
use crate::viewport::{DiffScrollbar, DiffScrollbarDrag, RenderedViewport, ViewportScrollInput};

mod diff_frame;
mod keys;
mod overlay;
mod reload;
mod search;

pub(crate) use keys::accepts_text_input;

use keys::is_ctrl_c;
use overlay::{AskAiPromptState, DiscardConfirmation, DiscardTarget, Overlay};
use reload::{bounded_hunk_index, initial_selected_hunk_index};

const MOUSE_WHEEL_STEP: usize = 3;
const HELP_OVERLAY_SCROLL_PAGE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Sidebar,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingLeftClick {
    Sidebar { index: usize },
    Diff { hunk_index: Option<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyAction {
    FocusedTarget,
    SelectedFileDiff,
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
    /// Index into the selected file's hunks.
    selected_hunk_index: Option<usize>,
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
    /// Deferred request for runtime to execute a configured shell command safely.
    custom_command_request: Option<CustomCommandBinding>,
    /// Deferred request for runtime to invoke OpenCode in read-only mode.
    ask_ai_request: Option<AskAiRequest>,
    /// Deferred request for runtime to cancel the active Ask AI task.
    ask_ai_cancel_request: bool,
    /// Deferred request for runtime to write explicit copied text.
    clipboard_request: Option<ClipboardRequest>,
    /// First rendered diff row visible in the diff pane.
    diff_scroll: usize,
    /// First file index considered for sidebar rendering.
    sidebar_scroll: usize,
    /// Rendered viewport geometry, row mapping, and render caches.
    viewport: RenderedViewport,
    /// Visible text rows and active drag-to-copy selection.
    text_selection: TextSelection,
    /// Deferred plain click target, cancelled if the press becomes a drag.
    pending_left_click: Option<PendingLeftClick>,
    /// Active mouse drag against the rendered diff scrollbar.
    diff_scrollbar_drag: Option<DiffScrollbarDrag>,
    /// Deferred request for runtime to open an external editor safely.
    editor_request: Option<EditorRequest>,
    /// Literal search prompt, query, matches, and active match.
    search: Search,
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
        let selected_hunk_index = initial_selected_hunk_index(&changeset);
        Self {
            source,
            changeset,
            live_error: None,
            live_notice: None,
            selected_file_index: 0,
            selected_hunk_index,
            focus: FocusPane::Sidebar,
            files_panel_visible: true,
            overlay: None,
            custom_commands: config.commands,
            keybinds: config.keybinds,
            theme: config.theme,
            custom_command_request: None,
            ask_ai_request: None,
            ask_ai_cancel_request: false,
            clipboard_request: None,
            diff_scroll: 0,
            sidebar_scroll: 0,
            viewport: RenderedViewport::new(file_count),
            text_selection: TextSelection::default(),
            pending_left_click: None,
            diff_scrollbar_drag: None,
            editor_request: None,
            search: Search::default(),
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

        let can_stage = self.can_stage();
        let row_counts = self
            .viewport
            .cached_sidebar_row_counts(content_width, can_stage, self.changeset.files.len(), || {
                rows::sidebar_row_counts(&self.changeset.files, content_width, can_stage, theme)
            })
            .to_vec();

        let rendered_rows = rows::sidebar_rows(SidebarRowsInput {
            files: &self.changeset.files,
            empty_message: self.empty_sidebar_message(),
            can_stage,
            selected_file_index: self.selected_file_index,
            sidebar_scroll: self.sidebar_scroll,
            row_counts: &row_counts,
            content_width,
            visible_height,
            theme,
            reviewed_files: &self.reviewed_files,
        });
        self.sidebar_scroll = rendered_rows.sidebar_scroll;
        self.viewport.begin_sidebar_rows();
        for record in rendered_rows.row_records {
            self.viewport
                .record_sidebar_rows(record.index, record.row_count);
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
        self.source.can_stage()
    }

    fn can_discard(&self) -> bool {
        self.source.can_discard()
    }

    fn stage_keybind_hint(&self) -> Option<&'static str> {
        if !self.can_stage() {
            return None;
        }

        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => Some("stage file"),
            FocusPane::Diff => Some("stage hunk"),
            FocusPane::Sidebar => None,
        }
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
        self.selected_file()?.hunks.get(self.selected_hunk_index?)
    }

    fn ensure_scroll_bounds(&mut self) {
        self.ensure_selected_hunk_bounds();
        let scrolls = self.viewport.clamped_scrolls(self.viewport_scroll_input());
        self.diff_scroll = scrolls.diff_scroll;
        self.sidebar_scroll = scrolls.sidebar_scroll;
    }

    fn ensure_selected_hunk_bounds(&mut self) {
        let Some(file) = self.selected_file() else {
            self.selected_hunk_index = None;
            return;
        };

        self.selected_hunk_index = bounded_hunk_index(file, self.selected_hunk_index);
    }

    fn viewport_scroll_input(&self) -> ViewportScrollInput<'_> {
        let selected_file = self.selected_file();
        ViewportScrollInput {
            diff_scroll: self.diff_scroll,
            sidebar_scroll: self.sidebar_scroll,
            selected_file_index: self.selected_file_index,
            file_count: self.changeset.files.len(),
            selected_file_id: selected_file.map(|file| file.id.as_str()),
            selected_file_line_count: selected_file.map_or(0, DiffFile::line_count),
        }
    }

    pub(crate) fn set_live_error(&mut self, error: String) {
        self.live_notice = None;
        self.live_error = Some(error);
    }

    pub(crate) fn set_live_notice(&mut self, notice: String) {
        self.live_error = None;
        self.live_notice = Some(notice);
    }

    pub(crate) fn take_editor_request(&mut self) -> Option<EditorRequest> {
        self.editor_request.take()
    }

    pub(crate) fn take_custom_command_request(&mut self) -> Option<CustomCommandBinding> {
        self.custom_command_request.take()
    }

    pub(crate) fn take_ask_ai_request(&mut self) -> Option<AskAiRequest> {
        self.ask_ai_request.take()
    }

    pub(crate) fn take_ask_ai_cancel_request(&mut self) -> bool {
        let requested = self.ask_ai_cancel_request;
        self.ask_ai_cancel_request = false;
        requested
    }

    pub(crate) fn take_clipboard_request(&mut self) -> Option<ClipboardRequest> {
        self.clipboard_request.take().or_else(|| {
            self.text_selection
                .take_clipboard_request()
                .map(|text| ClipboardRequest::new(text, "copied selected text"))
        })
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
        // A running command blocks all input, including ctrl-c, until it finishes.
        if matches!(self.overlay, Some(Overlay::CommandRunning { .. })) {
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
                // AskAi, ExplainCode, CopyFocused, and CopyFileDiff are
                // dispatched earlier in this method, so they never reach here.
                Some(
                    BuiltinAction::AskAi
                    | BuiltinAction::ExplainCode
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
        // A running command blocks all mouse input until it finishes.
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
            PendingLeftClick::Sidebar { index } => {
                self.focus = FocusPane::Sidebar;
                self.select_file(index);
            }
            PendingLeftClick::Diff { hunk_index } => {
                self.focus = FocusPane::Diff;
                if let Some(index) = hunk_index {
                    self.selected_hunk_index = Some(index);
                    self.center_selected_hunk();
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
        if let Some(index) = self.sidebar_index_at(column, row) {
            return Some(PendingLeftClick::Sidebar { index });
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
            FocusPane::Sidebar => self.select_file_by(direction, MOUSE_WHEEL_STEP),
            FocusPane::Diff => self.scroll_diff_by(direction, MOUSE_WHEEL_STEP),
        }
    }

    fn pane_at(&self, column: u16, row: u16) -> Option<FocusPane> {
        if self.is_sidebar_at(column, row) {
            return Some(FocusPane::Sidebar);
        }

        self.is_diff_at(column, row).then_some(FocusPane::Diff)
    }

    fn sidebar_index_at(&self, column: u16, row: u16) -> Option<usize> {
        self.viewport
            .sidebar_index_at(column, row, self.changeset.files.len())
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

        self.hunk_index_at_rendered_row(self.diff_scroll.saturating_add(diff_row))
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
        self.focus = if self.files_panel_visible {
            FocusPane::Sidebar
        } else {
            FocusPane::Diff
        };
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

    fn focused_ask_ai_context(&self) -> Option<AskAiContext> {
        let file_context = self.focus == FocusPane::Sidebar && self.files_panel_visible;
        let hunk_index = if file_context {
            None
        } else {
            self.selected_hunk_index
        };
        let selected_text = if file_context {
            None
        } else {
            self.text_selection.selected_text()
        };

        let file = self.selected_file()?;

        Some(AskAiContext::focused(
            self.source.ask_ai_review_mode(),
            self.changeset.title.clone(),
            self.changeset.source_label.clone(),
            file,
            hunk_index,
            selected_text,
        ))
    }

    fn open_ask_ai_prompt(&mut self) {
        let Some(context) = self.focused_ask_ai_context() else {
            self.live_error = Some("no selected file to ask about".to_string());
            return;
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

        self.ask_ai_request = Some(AskAiRequest::new(question, prompt.context));
    }

    fn queue_explain_code_request(&mut self) {
        let Some(context) = self.focused_ask_ai_context() else {
            self.live_error = Some("no selected file to explain".to_string());
            return;
        };

        self.focus = FocusPane::Diff;
        self.live_error = None;
        self.ask_ai_request = Some(AskAiRequest::explain_code(context));
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
        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => self.queue_selected_file_path_copy(),
            FocusPane::Diff => self.queue_selected_hunk_diff_copy(),
            FocusPane::Sidebar => self.set_copy_error("no visible file path to copy"),
        }
    }

    fn queue_selected_file_path_copy(&mut self) {
        let Some(file) = self.selected_file() else {
            self.set_copy_error("no selected file path to copy");
            return;
        };

        self.queue_clipboard_text(file.display_path().to_string(), "copied selected file path");
    }

    fn queue_selected_hunk_diff_copy(&mut self) {
        let Some(hunk_index) = self.focused_hunk_index() else {
            self.set_copy_error("no selected hunk to copy");
            return;
        };
        let Some(file) = self.selected_file() else {
            self.set_copy_error("no selected file to copy");
            return;
        };
        let Some(text) = patch::selected_hunk_patch(file, hunk_index) else {
            self.set_copy_error("no selected hunk diff to copy");
            return;
        };

        self.queue_clipboard_text(text, "copied selected hunk diff");
    }

    fn queue_selected_file_diff_copy(&mut self) {
        let Some(file) = self.selected_file() else {
            self.set_copy_error("no selected file to copy");
            return;
        };
        let Some(text) = patch::file_patch(file) else {
            self.set_copy_error("no selected file diff to copy");
            return;
        };

        self.queue_clipboard_text(text, "copied selected file diff");
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
        self.clipboard_request = Some(ClipboardRequest::new(text, success_message));
    }

    fn set_copy_error(&mut self, message: &'static str) {
        self.clipboard_request = None;
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
        self.custom_command_request = Some(command);
        true
    }

    fn move_by(&mut self, direction: VerticalDirection) {
        match self.focus {
            FocusPane::Sidebar => self.select_file_by(direction, 1),
            FocusPane::Diff => self.scroll_diff_by(direction, 1),
        }
    }

    fn select_file_by(&mut self, direction: VerticalDirection, amount: usize) {
        let max_index = self.changeset.files.len().saturating_sub(1);
        let index = direction.shift_clamped(self.selected_file_index, amount, max_index);
        self.select_file(index);
    }

    fn select_file(&mut self, index: usize) {
        if self.changeset.files.is_empty() {
            return;
        }

        self.selected_file_index = index.min(self.changeset.files.len() - 1);
        self.selected_hunk_index = self
            .selected_file()
            .and_then(|file| bounded_hunk_index(file, None));
        self.diff_scroll = 0;
        self.invalidate_search_matches();
    }

    fn scroll_diff_page(&mut self, direction: VerticalDirection) {
        self.scroll_diff_by(direction, self.viewport.diff_view_height());
    }

    fn scroll_diff_by(&mut self, direction: VerticalDirection, amount: usize) {
        self.diff_scroll = direction.shift(self.diff_scroll, amount);
        self.select_hunk_at_scroll();
    }

    fn scroll_diff_to(&mut self, scroll: usize) {
        self.diff_scroll = scroll;
        self.select_hunk_at_scroll();
    }

    fn scroll_diff_to_top(&mut self) {
        self.diff_scroll = 0;
        self.select_hunk_at_scroll();
    }

    fn scroll_diff_to_bottom(&mut self) {
        self.diff_scroll = usize::MAX;
        self.selected_hunk_index = self
            .selected_file()
            .and_then(|file| file.hunks.len().checked_sub(1));
    }

    fn jump_hunk(&mut self, direction: VerticalDirection) {
        let Some(hunk_count) = self.selected_file().map(|file| file.hunks.len()) else {
            return;
        };
        if hunk_count == 0 {
            self.selected_hunk_index = None;
            return;
        }

        let current = self
            .selected_hunk_index
            .or_else(|| self.hunk_index_at_rendered_row(self.diff_scroll))
            .unwrap_or(0)
            .min(hunk_count - 1);
        let target = direction.shift_clamped(current, 1, hunk_count - 1);

        self.selected_hunk_index = Some(target);
        self.center_selected_hunk();
    }

    fn center_selected_hunk(&mut self) {
        let Some(index) = self.selected_hunk_index else {
            return;
        };
        let Some((file_id, hunk_count)) = self
            .selected_file()
            .map(|file| (file.id.clone(), file.hunks.len()))
        else {
            return;
        };
        if index >= hunk_count {
            return;
        }

        if let Some(offset) =
            self.viewport
                .hunk_offset(self.selected_file_index, file_id.as_str(), index)
        {
            self.diff_scroll = offset.saturating_sub(self.viewport.diff_view_height() / 2);
        }
    }

    fn select_hunk_at_scroll(&mut self) {
        self.selected_hunk_index = self.hunk_index_at_rendered_row(self.diff_scroll);
    }

    fn hunk_index_at_rendered_row(&self, rendered_row: usize) -> Option<usize> {
        let file = self.selected_file()?;
        self.viewport.hunk_index_at(
            self.selected_file_index,
            file.id.as_str(),
            rendered_row,
            file.hunks.len(),
        )
    }

    fn toggle_selected_staging(&mut self) {
        if !self.can_stage() {
            return;
        }

        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => self.toggle_selected_file_staging(),
            FocusPane::Diff => self.toggle_selected_hunk_staging(),
            FocusPane::Sidebar => {}
        }
    }

    fn toggle_selected_file_staging(&mut self) {
        let Some(file) = self.selected_file() else {
            return;
        };

        let path = file.display_path().to_string();
        match self.source.toggle_staging_for_file(&path) {
            Ok(Some(reloaded_changeset)) => {
                self.apply_reloaded_changeset(reloaded_changeset, false)
            }
            Ok(None) => {}
            Err(error) => self.live_error = Some(format!("staging failed: {error}")),
        }
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

    fn toggle_selected_hunk_staging(&mut self) {
        let Some(hunk_index) = self.focused_hunk_index() else {
            self.live_error = Some("no selected hunk to stage".to_string());
            return;
        };
        let Some(file) = self.selected_file().cloned() else {
            self.live_error = Some("no selected file to stage".to_string());
            return;
        };

        match self.source.toggle_staging_for_hunk(&file, hunk_index) {
            Ok(Some(reloaded_changeset)) => self.apply_reloaded_changeset(reloaded_changeset, true),
            Ok(None) => {}
            Err(error) => self.live_error = Some(format!("hunk staging failed: {error}")),
        }
    }

    fn focused_hunk_index(&self) -> Option<usize> {
        let file = self.selected_file()?;
        let index = self.selected_hunk_index?;

        (index < file.hunks.len()).then_some(index)
    }

    fn request_selected_discard(&mut self) {
        if !self.can_discard() {
            return;
        }

        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => self.request_selected_file_discard(),
            FocusPane::Diff => self.request_selected_hunk_discard(),
            FocusPane::Sidebar => {}
        }
    }

    fn request_selected_file_discard(&mut self) {
        let Some(file) = self.selected_file() else {
            self.live_error = Some("no selected file to discard".to_string());
            return;
        };
        let path = file.display_path().to_string();

        self.live_error = None;
        self.overlay = Some(Overlay::Discard(DiscardConfirmation {
            target: DiscardTarget::File {
                file_index: self.selected_file_index,
                path,
            },
        }));
    }

    fn request_selected_hunk_discard(&mut self) {
        let Some(hunk_index) = self.focused_hunk_index() else {
            self.live_error = Some("no selected hunk to discard".to_string());
            return;
        };
        let Some(file) = self.selected_file() else {
            self.live_error = Some("no selected file to discard".to_string());
            return;
        };
        let path = file.display_path().to_string();

        self.live_error = None;
        self.overlay = Some(Overlay::Discard(DiscardConfirmation {
            target: DiscardTarget::Hunk {
                file_index: self.selected_file_index,
                hunk_index,
                path,
            },
        }));
    }

    fn execute_pending_discard(&mut self) {
        let Some(Overlay::Discard(confirmation)) = self.overlay.take() else {
            return;
        };

        let result = match confirmation.target {
            DiscardTarget::File { file_index, path } => {
                if self.confirmed_file(file_index, &path).is_none() {
                    return;
                }
                self.source.discard_file(&path)
            }
            DiscardTarget::Hunk {
                file_index,
                hunk_index,
                path,
            } => {
                let Some(file) = self.confirmed_file(file_index, &path) else {
                    return;
                };
                self.source.discard_hunk(&file, hunk_index)
            }
        };

        match result {
            Ok(Some(reloaded_changeset)) => self.apply_reloaded_changeset(reloaded_changeset, true),
            Ok(None) => {}
            Err(error) => self.live_error = Some(format!("discard failed: {error}")),
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
        self.editor_request = None;
        let Some(file) = self.selected_file() else {
            self.live_error = Some("no selected file to open".to_string());
            return;
        };

        match self.source.editor_request(file) {
            Ok(request) => {
                self.live_error = None;
                self.editor_request = Some(request);
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

#[cfg(test)]
fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[cfg(test)]
mod tests;
