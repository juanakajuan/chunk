//! Terminal application session state.
//!
//! `App` owns selection, focus, scroll state, and live reload errors. Review
//! source behavior lives in `review_source`; terminal and watch orchestration
//! live in `runtime`; rendered row preparation lives here while `ui` draws
//! Ratatui widgets.

use std::path::PathBuf;

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::ask_ai::{AskAiContext, AskAiRequest};
use crate::config::AppConfig;
use crate::custom_command::CustomCommandBinding;
use crate::editor::EditorRequest;
use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::review_source::{LoadedReview, ReviewSource};
use crate::rows::{self, SidebarRowsInput};
use crate::scroll_text::VerticalDirection;
use crate::search::Search;
use crate::selection::TextSelection;
use crate::theme::Theme;
use crate::viewport::{
    DiffLayoutMetrics, DiffLayoutRequest, DiffRenderRequest, DiffScrollbar, DiffScrollbarDrag,
    RenderedViewport, ViewportScrollInput,
};

mod keys;
mod overlay;
mod reload;

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

#[derive(Debug)]
pub(crate) struct App {
    /// Review source behavior for this session.
    source: ReviewSource,
    /// Current diff data being reviewed.
    changeset: Changeset,
    /// Last live reload/watch error, rendered above the diff when present.
    live_error: Option<String>,
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
    /// Deferred request for runtime to execute a configured shell command safely.
    custom_command_request: Option<CustomCommandBinding>,
    /// Deferred request for runtime to invoke OpenCode in read-only mode.
    ask_ai_request: Option<AskAiRequest>,
    /// Deferred request for runtime to cancel the active Ask AI task.
    ask_ai_cancel_request: bool,
    /// First rendered diff row visible in the diff pane.
    diff_scroll: usize,
    /// Rendered live-status rows above the diff rows in the diff pane.
    diff_status_rows: usize,
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
            selected_file_index: 0,
            selected_hunk_index,
            focus: FocusPane::Sidebar,
            files_panel_visible: true,
            overlay: None,
            custom_commands: config.commands,
            custom_command_request: None,
            ask_ai_request: None,
            ask_ai_cancel_request: false,
            diff_scroll: 0,
            diff_status_rows: 0,
            sidebar_scroll: 0,
            viewport: RenderedViewport::new(file_count),
            text_selection: TextSelection::default(),
            pending_left_click: None,
            diff_scrollbar_drag: None,
            editor_request: None,
            search: Search::default(),
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

    pub(crate) fn diff_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        if self.ask_ai_output().is_some() {
            return self.ask_ai_output_pane_rows(area, content_width, visible_height, theme);
        }
        if self.command_output().is_some() {
            return self.command_output_pane_rows(area, content_width, visible_height, theme);
        }

        let title = format!(" {} ", rows::changeset_title(&self.changeset));
        let mut lines = rows::live_status_lines(self.live_error.as_deref(), content_width, theme);
        let running = self.command_running();
        lines.extend(rows::custom_command_running_lines(
            running.map(|(binding, _)| binding),
            running.map_or(0, |(_, frame)| frame),
            content_width,
            theme,
        ));
        lines.extend(rows::ask_ai_prompt_lines(
            self.ask_ai_prompt().map(|prompt| prompt.input.as_str()),
            content_width,
            theme,
        ));
        let ask_ai_running = self.ask_ai_running();
        lines.extend(rows::ask_ai_running_lines(
            ask_ai_running.map(|(question, _, _)| question),
            ask_ai_running.map_or(0, |(_, frame, _)| frame),
            ask_ai_running.is_some_and(|(_, _, cancelling)| cancelling),
            content_width,
            theme,
        ));
        lines.extend(self.discard_status_lines(content_width, theme));

        let provisional_search_lines =
            rows::search_status_lines(self.search.status(), content_width, theme);
        let provisional_visible_diff_height =
            visible_height.saturating_sub(lines.len() + provisional_search_lines.len());
        let mut diff_content_width =
            self.diff_content_width(content_width, provisional_visible_diff_height, theme);

        let pending_search_scroll = if self.search.active_query().is_some() {
            self.viewport
                .begin_diff(area, provisional_visible_diff_height);
            self.ensure_scroll_bounds();
            self.ensure_selected_diff_cache(
                diff_content_width,
                provisional_visible_diff_height,
                theme,
            )
        } else {
            false
        };

        lines.extend(rows::search_status_lines(
            self.search.status(),
            content_width,
            theme,
        ));

        self.diff_status_rows = lines.len();
        let visible_diff_height = visible_height.saturating_sub(self.diff_status_rows);
        diff_content_width = self.diff_content_width(content_width, visible_diff_height, theme);
        let total_diff_rows = self.selected_diff_line_count(diff_content_width, theme);
        self.viewport.begin_diff(area, visible_diff_height);
        self.viewport.set_diff_scrollbar(self.diff_scrollbar(
            area,
            content_width,
            visible_diff_height,
            total_diff_rows,
        ));
        self.ensure_scroll_bounds();

        if pending_search_scroll {
            self.scroll_active_search_match();
            self.select_hunk_at_scroll();
            self.ensure_scroll_bounds();
        }

        if visible_diff_height > 0 {
            lines.extend(self.selected_diff_lines(diff_content_width, visible_diff_height, theme));
        }
        lines.truncate(visible_height);
        self.viewport.set_diff_scrollbar(self.diff_scrollbar(
            area,
            content_width,
            visible_diff_height,
            total_diff_rows,
        ));

        let lines = self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: self.viewport.diff_scrollbar().cloned(),
        }
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
            return rows::ask_ai_output_keybind_bar_line(theme);
        }
        if self.ask_ai_prompt().is_some() {
            return rows::ask_ai_prompt_keybind_bar_line(theme);
        }
        if self.ask_ai_running().is_some() {
            return rows::ask_ai_running_keybind_bar_line(theme);
        }
        if self.command_output().is_some() {
            return rows::custom_command_output_keybind_bar_line(theme);
        }

        rows::keybind_bar_line(self.files_panel_visible, self.stage_keybind_hint(), theme)
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

    fn diff_content_width(
        &mut self,
        content_width: usize,
        visible_diff_height: usize,
        theme: Theme,
    ) -> usize {
        if content_width > 1
            && visible_diff_height > 0
            && self.selected_diff_line_count(content_width, theme) > visible_diff_height
        {
            content_width - 1
        } else {
            content_width
        }
    }

    fn selected_diff_line_count(&mut self, content_width: usize, theme: Theme) -> usize {
        self.selected_diff_layout_metrics(content_width, theme)
            .map_or(0, |metrics| metrics.total_rows)
    }

    fn selected_diff_layout_metrics(
        &mut self,
        content_width: usize,
        theme: Theme,
    ) -> Option<&DiffLayoutMetrics> {
        self.viewport
            .ensure_diff_cache_len(self.changeset.files.len());

        let selected_file_index = self.selected_file_index;
        let file_id = self.changeset.files.get(selected_file_index)?.id.clone();
        let request = DiffLayoutRequest {
            file_index: selected_file_index,
            file_id: file_id.as_str(),
            content_width,
            can_stage: self.can_stage(),
        };

        if self.viewport.diff_layout_metrics(request).is_none() {
            let file = &self.changeset.files[selected_file_index];
            let counts = rows::diff_layout_counts(file, content_width, theme, request.can_stage);
            self.viewport.cache_diff_layout_metrics(
                request,
                DiffLayoutMetrics::new(counts.total_rows, counts.hunk_offsets),
            );
        }

        self.viewport.diff_layout_metrics(request)
    }

    fn diff_scrollbar(
        &self,
        area: Rect,
        content_width: usize,
        visible_diff_height: usize,
        total_diff_rows: usize,
    ) -> Option<DiffScrollbar> {
        if content_width <= 1 || visible_diff_height == 0 || total_diff_rows <= visible_diff_height
        {
            return None;
        }

        let file = self.selected_file()?;
        let scrollbar_area = Rect {
            x: area.x.saturating_add(area.width.saturating_sub(2)),
            y: area
                .y
                .saturating_add(1)
                .saturating_add(saturating_u16(self.diff_status_rows)),
            width: 1,
            height: saturating_u16(visible_diff_height),
        };

        Some(DiffScrollbar::new(
            scrollbar_area,
            self.selected_file_index,
            file.id.clone(),
            total_diff_rows,
            visible_diff_height,
            self.diff_scroll,
        ))
    }

    fn selected_diff_lines(
        &mut self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.ensure_selected_diff_cache(content_width, visible_height, theme);
        self.visible_selected_diff_lines(content_width, visible_height, theme)
    }

    fn ensure_selected_diff_cache(
        &mut self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> bool {
        self.viewport
            .ensure_diff_cache_len(self.changeset.files.len());

        let selected_file_index = self.selected_file_index;
        let can_stage = self.can_stage();
        if selected_file_index >= self.changeset.files.len() {
            self.search.clear_rendered_matches();
            return false;
        }

        let target_rows = self.diff_render_target_rows(visible_height);
        let hunk_offsets = self
            .selected_diff_layout_metrics(content_width, theme)
            .map(|metrics| metrics.hunk_offsets.clone())
            .unwrap_or_default();

        let file_id = self.changeset.files[selected_file_index].id.clone();
        let request = DiffRenderRequest {
            file_index: selected_file_index,
            file_id: file_id.as_str(),
            content_width,
            syntax_palette: theme.syntax,
            can_stage,
            requested_rows: target_rows,
        };

        // Split borrows so the render seam can load source snapshots and render
        // rows while the viewport owns the cache; source snapshots load only
        // when the viewport actually invokes `render`.
        let Self {
            viewport,
            changeset,
            source,
            ..
        } = self;
        viewport.ensure_diff_lines(request, hunk_offsets, |render_target| {
            if let Some(file) = changeset.files.get_mut(selected_file_index) {
                source.load_source_snapshots(file);
            }
            let file = &changeset.files[selected_file_index];
            let rendered =
                rows::diff_lines_until(file, content_width, theme, can_stage, None, render_target);
            (rendered.lines, rendered.complete)
        });

        let pending_search_scroll = self.refresh_search_matches(selected_file_index);
        self.ensure_scroll_bounds();

        pending_search_scroll
    }

    fn diff_render_target_rows(&self, visible_height: usize) -> usize {
        if self.search.active_query().is_some() {
            return usize::MAX;
        }

        self.diff_scroll
            .saturating_add(visible_height)
            .saturating_add(rows::DIFF_PREFETCH_ROWS)
    }

    fn refresh_search_matches(&mut self, selected_file_index: usize) -> bool {
        let Some(file_id) = self
            .changeset
            .files
            .get(selected_file_index)
            .map(|file| file.id.as_str())
        else {
            self.search.clear_rendered_matches();
            return false;
        };

        let Some(lines) = self.viewport.diff_lines(selected_file_index, file_id) else {
            self.search.clear_rendered_matches();
            return false;
        };

        self.search.refresh_matches(file_id, lines)
    }

    fn visible_selected_diff_lines(
        &self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        if self.selected_file_index >= self.changeset.files.len() {
            return rows::no_diff_lines(self.no_diff_message(), content_width, theme);
        }

        let mut lines = self.viewport.visible_diff_lines(
            self.selected_file_index,
            self.diff_scroll,
            visible_height,
        );
        self.apply_selected_hunk_style(&mut lines, content_width, theme);
        self.search.highlight(lines, self.diff_scroll, theme)
    }

    fn command_output_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        self.viewport.begin_diff(area, visible_height);
        self.diff_status_rows = 0;

        let output = self
            .command_output_mut()
            .expect("command output pane requires command output state");
        let title = format!(" Command: {} ", output.result.label());
        let all_lines = rows::custom_command_output_lines(&output.result, content_width, theme);
        output.scroll.sync(all_lines.len(), visible_height);
        let lines = output.scroll.visible(all_lines);
        let lines = self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: None,
        }
    }

    fn ask_ai_output_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        self.viewport.begin_diff(area, visible_height);
        self.diff_status_rows = 0;

        let output = self
            .ask_ai_output_mut()
            .expect("Ask AI output pane requires output state");
        let title = format!(" Ask AI: {} ", output.result.context_summary());
        let all_lines = rows::ask_ai_output_lines(&output.result, content_width, theme);
        output.scroll.sync(all_lines.len(), visible_height);
        let lines = output.scroll.visible(all_lines);
        let lines = self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: None,
        }
    }

    fn apply_selected_hunk_style(
        &self,
        lines: &mut [Line<'static>],
        content_width: usize,
        theme: Theme,
    ) {
        let Some(selected_hunk_index) = self.selected_hunk_index else {
            return;
        };
        let Some(file) = self.selected_file() else {
            return;
        };
        let Some(hunk) = file.hunks.get(selected_hunk_index) else {
            return;
        };
        let Some(hunk_offset) = self.viewport.hunk_offset(
            self.selected_file_index,
            file.id.as_str(),
            selected_hunk_index,
        ) else {
            return;
        };

        let visible_start = self.diff_scroll;
        let visible_end = visible_start.saturating_add(lines.len());
        let header_rows =
            rows::selected_hunk_header_rows(hunk, content_width, theme, self.can_stage());
        for (header_row_offset, header_row) in header_rows.into_iter().enumerate() {
            let rendered_row = hunk_offset.saturating_add(header_row_offset);
            if rendered_row < visible_start || rendered_row >= visible_end {
                continue;
            }

            lines[rendered_row - visible_start] = header_row;
        }
    }

    pub(crate) fn set_live_error(&mut self, error: String) {
        self.live_error = Some(error);
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

    pub(crate) fn take_clipboard_request(&mut self) -> Option<String> {
        self.text_selection.take_clipboard_request()
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

        if self.search.is_prompt_open() {
            self.search.handle_prompt_key(key);
            self.ensure_scroll_bounds();
            return Ok(true);
        }

        if self.queue_custom_command_for_key(key) {
            return Ok(true);
        }

        match key.code {
            KeyCode::Char('q') if accepts_text_input(key) => return Ok(false),
            KeyCode::Esc => self.search.clear_query(),
            KeyCode::Char('?') if accepts_text_input(key) => self.toggle_help_overlay(),

            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Char('f') => self.toggle_files_panel(),
            KeyCode::Left if self.files_panel_visible => self.focus = FocusPane::Sidebar,
            KeyCode::Right | KeyCode::Enter => self.focus = FocusPane::Diff,

            KeyCode::Char('/') if accepts_text_input(key) => self.open_search_prompt(),

            KeyCode::Char('j') => self.move_by(VerticalDirection::Down),
            KeyCode::Char('k') => self.move_by(VerticalDirection::Up),

            KeyCode::Char('n') => self.jump_by(VerticalDirection::Down),
            KeyCode::Char('N') => self.jump_by(VerticalDirection::Up),

            KeyCode::Home | KeyCode::Char('g') => self.scroll_diff_to_top(),
            KeyCode::End | KeyCode::Char('G') => self.scroll_diff_to_bottom(),

            KeyCode::Char(' ') => self.toggle_selected_staging(),
            KeyCode::Char('d') if accepts_text_input(key) => self.request_selected_discard(),
            KeyCode::Char('e') => self.queue_selected_file_editor_request(),

            KeyCode::PageDown => self.scroll_diff_page(VerticalDirection::Down),
            KeyCode::PageUp => self.scroll_diff_page(VerticalDirection::Up),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_page(VerticalDirection::Down)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_page(VerticalDirection::Up)
            }
            _ => {}
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
        let diff_row = visible_row.checked_sub(self.diff_status_rows)?;

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

    fn open_search_prompt(&mut self) {
        self.focus = FocusPane::Diff;
        self.search.open_prompt();
    }

    fn should_open_ask_ai_prompt(&self, key: KeyEvent) -> bool {
        self.overlay.is_none()
            && !self.search.is_prompt_open()
            && key.code == KeyCode::Char('a')
            && accepts_text_input(key)
    }

    fn should_queue_explain_code_request(&self, key: KeyEvent) -> bool {
        self.overlay.is_none()
            && !self.search.is_prompt_open()
            && key.code == KeyCode::Char('x')
            && accepts_text_input(key)
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
        self.search.invalidate_matches();
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

    fn jump_by(&mut self, direction: VerticalDirection) {
        if self.search.active_query().is_some() {
            self.jump_search_match(direction);
        } else {
            self.jump_hunk(direction);
        }
    }

    fn jump_search_match(&mut self, direction: VerticalDirection) {
        if self
            .search
            .advance_match(matches!(direction, VerticalDirection::Down))
        {
            self.scroll_active_search_match();
            self.select_hunk_at_scroll();
        }
    }

    fn scroll_active_search_match(&mut self) {
        let Some(active_row) = self.search.active_match_row() else {
            return;
        };

        self.diff_scroll = active_row.saturating_sub(self.viewport.diff_view_height() / 2);
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
