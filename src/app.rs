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

use crate::ask_ai::{AskAiContext, AskAiRequest, AskAiResult};
use crate::config::AppConfig;
use crate::custom_command::{CustomCommandBinding, CustomCommandResult};
use crate::editor::EditorRequest;
use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::review_source::{LoadedReview, ReviewSource};
use crate::rows::{self, SidebarRowsInput};
use crate::search::Search;
use crate::selection::TextSelection;
use crate::theme::Theme;
use crate::viewport::{
    DiffLayoutMetrics, DiffLayoutRequest, DiffRenderRequest, DiffScrollbar, DiffScrollbarDrag,
    RenderedViewport, ViewportScrollInput,
};

const MOUSE_WHEEL_STEP: usize = 3;
const HELP_OVERLAY_SCROLL_PAGE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Sidebar,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerticalDirection {
    Down,
    Up,
}

impl VerticalDirection {
    fn shift(self, value: usize, amount: usize) -> usize {
        match self {
            Self::Down => value.saturating_add(amount),
            Self::Up => value.saturating_sub(amount),
        }
    }

    fn shift_clamped(self, value: usize, amount: usize, max: usize) -> usize {
        self.shift(value, amount).min(max)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollKeyAction {
    Line(VerticalDirection),
    Page(VerticalDirection),
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingLeftClick {
    Sidebar { index: usize },
    Diff { hunk_index: Option<usize> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandOutputState {
    result: CustomCommandResult,
    scroll: usize,
    rendered_row_count: usize,
    visible_height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AskAiPromptState {
    context: AskAiContext,
    input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AskAiOutputState {
    result: AskAiResult,
    scroll: usize,
    rendered_row_count: usize,
    visible_height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscardConfirmation {
    target: DiscardTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiscardTarget {
    File {
        file_index: usize,
        path: String,
    },
    Hunk {
        file_index: usize,
        hunk_index: usize,
        path: String,
    },
}

impl DiscardConfirmation {
    fn prompt(&self) -> String {
        match &self.target {
            DiscardTarget::File { path, .. } => {
                format!("Discard worktree changes in {path}?")
            }
            DiscardTarget::Hunk {
                hunk_index, path, ..
            } => format!("Discard hunk {} in {path}?", hunk_index + 1),
        }
    }
}

/// The single modal overlay active over the diff view, if any.
///
/// At most one overlay can be active at a time. Holding them in one value makes
/// that exclusivity a type invariant instead of a guard-clause ordering that
/// would otherwise be re-derived across `handle_key`, `handle_mouse`, and the
/// renderer. The literal search prompt is intentionally not an overlay: it
/// captures keystrokes only, leaves mouse handling identical to the normal diff
/// view, and its persistent query/match state belongs to the `search` module.
#[derive(Debug)]
enum Overlay {
    /// Keymap help modal; owns the scroll offset for keymaps taller than the modal.
    Help { scroll: usize },
    /// Pending destructive worktree discard awaiting y/n confirmation.
    Discard(DiscardConfirmation),
    /// A custom command is running; all input is swallowed until runtime delivers output.
    CommandRunning {
        binding: CustomCommandBinding,
        spinner_frame: usize,
    },
    /// Completed custom command output shown in the diff pane.
    CommandOutput(CommandOutputState),
    /// Free-form Ask AI prompt; owns input until submitted.
    AskAiPrompt(AskAiPromptState),
    /// Ask AI request is running in the background.
    AskAiRunning {
        question: String,
        spinner_frame: usize,
        cancelling: bool,
    },
    /// Completed Ask AI answer shown in the diff pane.
    AskAiOutput(AskAiOutputState),
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

    pub(crate) fn help_overlay_visible(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Help { .. }))
    }

    pub(crate) fn help_overlay_scroll(&self) -> usize {
        match &self.overlay {
            Some(Overlay::Help { scroll }) => *scroll,
            _ => 0,
        }
    }

    pub(crate) fn clamp_help_overlay_scroll(&mut self, line_count: usize, visible_height: usize) {
        if let Some(Overlay::Help { scroll }) = &mut self.overlay {
            *scroll = (*scroll).min(line_count.saturating_sub(visible_height));
        }
    }

    fn set_help_overlay_scroll(&mut self, value: usize) {
        if let Some(Overlay::Help { scroll }) = &mut self.overlay {
            *scroll = value;
        }
    }

    fn command_output(&self) -> Option<&CommandOutputState> {
        match &self.overlay {
            Some(Overlay::CommandOutput(output)) => Some(output),
            _ => None,
        }
    }

    fn command_output_mut(&mut self) -> Option<&mut CommandOutputState> {
        match &mut self.overlay {
            Some(Overlay::CommandOutput(output)) => Some(output),
            _ => None,
        }
    }

    fn command_running(&self) -> Option<(&CustomCommandBinding, usize)> {
        match &self.overlay {
            Some(Overlay::CommandRunning {
                binding,
                spinner_frame,
            }) => Some((binding, *spinner_frame)),
            _ => None,
        }
    }

    fn ask_ai_prompt(&self) -> Option<&AskAiPromptState> {
        match &self.overlay {
            Some(Overlay::AskAiPrompt(prompt)) => Some(prompt),
            _ => None,
        }
    }

    fn ask_ai_prompt_mut(&mut self) -> Option<&mut AskAiPromptState> {
        match &mut self.overlay {
            Some(Overlay::AskAiPrompt(prompt)) => Some(prompt),
            _ => None,
        }
    }

    fn ask_ai_running(&self) -> Option<(&str, usize, bool)> {
        match &self.overlay {
            Some(Overlay::AskAiRunning {
                question,
                spinner_frame,
                cancelling,
            }) => Some((question.as_str(), *spinner_frame, *cancelling)),
            _ => None,
        }
    }

    fn ask_ai_output(&self) -> Option<&AskAiOutputState> {
        match &self.overlay {
            Some(Overlay::AskAiOutput(output)) => Some(output),
            _ => None,
        }
    }

    fn ask_ai_output_mut(&mut self) -> Option<&mut AskAiOutputState> {
        match &mut self.overlay {
            Some(Overlay::AskAiOutput(output)) => Some(output),
            _ => None,
        }
    }

    #[cfg(test)]
    fn discard_target(&self) -> Option<&DiscardTarget> {
        match &self.overlay {
            Some(Overlay::Discard(confirmation)) => Some(&confirmation.target),
            _ => None,
        }
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

    fn discard_status_lines(&self, content_width: usize, theme: Theme) -> Vec<Line<'static>> {
        let prompt = match &self.overlay {
            Some(Overlay::Discard(confirmation)) => Some(confirmation.prompt()),
            _ => None,
        };
        rows::discard_status_lines(prompt.as_deref(), content_width, theme)
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
        output.rendered_row_count = all_lines.len();
        output.visible_height = visible_height;
        clamp_command_output_scroll(output);
        let scroll = output.scroll;
        let lines = all_lines
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect();
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
        output.rendered_row_count = all_lines.len();
        output.visible_height = visible_height;
        clamp_ask_ai_output_scroll(output);
        let scroll = output.scroll;
        let lines = all_lines
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect();
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

    pub(crate) fn set_custom_command_running(&mut self, command: &CustomCommandBinding) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::CommandRunning {
            binding: command.clone(),
            spinner_frame: 0,
        });
    }

    pub(crate) fn advance_custom_command_spinner(&mut self) {
        if let Some(Overlay::CommandRunning { spinner_frame, .. }) = &mut self.overlay {
            *spinner_frame = spinner_frame.wrapping_add(1);
        }
    }

    pub(crate) fn set_custom_command_result(&mut self, result: CustomCommandResult) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::CommandOutput(CommandOutputState {
            result,
            scroll: 0,
            rendered_row_count: 0,
            visible_height: 0,
        }));
    }

    pub(crate) fn set_ask_ai_running(&mut self, request: &AskAiRequest) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::AskAiRunning {
            question: request.question().to_string(),
            spinner_frame: 0,
            cancelling: false,
        });
    }

    pub(crate) fn advance_ask_ai_spinner(&mut self) {
        if let Some(Overlay::AskAiRunning { spinner_frame, .. }) = &mut self.overlay {
            *spinner_frame = spinner_frame.wrapping_add(1);
        }
    }

    pub(crate) fn set_ask_ai_result(&mut self, result: AskAiResult) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::AskAiOutput(AskAiOutputState {
            result,
            scroll: 0,
            rendered_row_count: 0,
            visible_height: 0,
        }));
    }

    pub(crate) fn reload_review_source(&mut self, preserve_scroll: bool) {
        match self.source.reload() {
            Ok(changeset) => self.apply_reloaded_changeset(changeset, preserve_scroll),
            Err(error) => self.live_error = Some(format!("reload failed: {error}")),
        }
    }

    fn apply_reloaded_changeset(&mut self, changeset: Changeset, preserve_scroll: bool) {
        let previous_identity = self.selected_file().map(file_identity);
        let previous_hunk_identity = self.selected_hunk().map(hunk_identity);
        let previous_hunk_index = self.selected_hunk_index;
        let previous_index = self.selected_file_index;
        let previous_scroll = self.diff_scroll;
        let reselected_file_index = previous_identity
            .as_deref()
            .and_then(|identity| find_file_index(&changeset, identity));
        let fallback_index = previous_index.min(changeset.files.len().saturating_sub(1));
        let kept_selection = reselected_file_index.is_some();
        let selected_file_index = reselected_file_index.unwrap_or(fallback_index);

        self.changeset = changeset;
        self.live_error = None;
        if matches!(self.overlay, Some(Overlay::Discard(_))) {
            self.overlay = None;
        }
        self.text_selection.clear();
        self.selected_file_index = selected_file_index;
        self.selected_hunk_index = reloaded_hunk_index(
            self.changeset.files.get(selected_file_index),
            kept_selection,
            previous_hunk_identity,
            previous_hunk_index,
        );
        self.diff_scroll = if preserve_scroll && kept_selection {
            previous_scroll
        } else {
            0
        };
        self.clear_render_caches();
        self.search.invalidate_matches();
        self.ensure_scroll_bounds();
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

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay {
            Some(Overlay::Help { .. }) => self.handle_help_overlay_key(key),
            Some(Overlay::Discard(_)) => self.handle_discard_confirmation_key(key),
            Some(Overlay::CommandOutput(_)) => self.handle_command_output_key(key),
            Some(Overlay::AskAiPrompt(_)) => self.handle_ask_ai_prompt_key(key),
            Some(Overlay::AskAiRunning { .. }) => self.handle_ask_ai_running_key(key),
            Some(Overlay::AskAiOutput(_)) => self.handle_ask_ai_output_key(key),
            Some(Overlay::CommandRunning { .. }) | None => {}
        }
    }

    fn handle_overlay_mouse(&mut self, mouse: MouseEvent) {
        match self.overlay {
            Some(Overlay::Help { .. }) => self.handle_help_overlay_mouse(mouse),
            Some(Overlay::CommandOutput(_)) => self.handle_command_output_mouse(mouse),
            Some(Overlay::AskAiOutput(_)) => self.handle_ask_ai_output_mouse(mouse),
            Some(Overlay::Discard(_))
            | Some(Overlay::CommandRunning { .. })
            | Some(Overlay::AskAiPrompt(_))
            | Some(Overlay::AskAiRunning { .. })
            | None => {}
        }
    }

    fn handle_help_overlay_key(&mut self, key: KeyEvent) {
        if closes_help_overlay(key) {
            self.overlay = None;
            return;
        }

        match scroll_key_action(key) {
            Some(ScrollKeyAction::Line(direction)) => self.scroll_help_overlay_by(direction, 1),
            Some(ScrollKeyAction::Page(direction)) => {
                self.scroll_help_overlay_by(direction, HELP_OVERLAY_SCROLL_PAGE)
            }
            Some(ScrollKeyAction::Top) => self.set_help_overlay_scroll(0),
            Some(ScrollKeyAction::Bottom) => self.set_help_overlay_scroll(usize::MAX),
            None => {}
        }
    }

    fn handle_help_overlay_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            app.scroll_help_overlay_by(direction, MOUSE_WHEEL_STEP);
        });
    }

    fn handle_discard_confirmation_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.execute_pending_discard(),
            KeyCode::Char('y') if accepts_text_input(key) => self.execute_pending_discard(),
            KeyCode::Esc => self.overlay = None,
            KeyCode::Char('n') if accepts_text_input(key) => self.overlay = None,
            _ => {}
        }
    }

    fn handle_command_output_key(&mut self, key: KeyEvent) {
        if closes_command_output(key) {
            self.overlay = None;
            return;
        }

        match scroll_key_action(key) {
            Some(ScrollKeyAction::Line(direction)) => self.scroll_command_output_by(direction, 1),
            Some(ScrollKeyAction::Page(direction)) => {
                self.scroll_command_output_by(direction, self.command_output_page())
            }
            Some(ScrollKeyAction::Top) => self.scroll_command_output_to_top(),
            Some(ScrollKeyAction::Bottom) => self.scroll_command_output_to_bottom(),
            None => {}
        }
    }

    fn handle_command_output_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            app.scroll_command_output_by(direction, MOUSE_WHEEL_STEP);
        });
    }

    fn handle_ask_ai_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Enter => self.submit_ask_ai_prompt(),
            KeyCode::Backspace => {
                if let Some(prompt) = self.ask_ai_prompt_mut() {
                    prompt.input.pop();
                }
            }
            KeyCode::Char(value) if accepts_text_input(key) => {
                if let Some(prompt) = self.ask_ai_prompt_mut() {
                    prompt.input.push(value);
                }
            }
            _ => {}
        }
    }

    fn handle_ask_ai_running_key(&mut self, key: KeyEvent) {
        if !closes_ask_ai_running(key) {
            return;
        }

        if let Some(Overlay::AskAiRunning {
            cancelling,
            question: _,
            spinner_frame: _,
        }) = &mut self.overlay
            && !*cancelling
        {
            *cancelling = true;
            self.ask_ai_cancel_request = true;
        }
    }

    fn handle_ask_ai_output_key(&mut self, key: KeyEvent) {
        if closes_ask_ai_output(key) {
            self.overlay = None;
            return;
        }

        match scroll_key_action(key) {
            Some(ScrollKeyAction::Line(direction)) => self.scroll_ask_ai_output_by(direction, 1),
            Some(ScrollKeyAction::Page(direction)) => {
                self.scroll_ask_ai_output_by(direction, self.ask_ai_output_page())
            }
            Some(ScrollKeyAction::Top) => self.scroll_ask_ai_output_to_top(),
            Some(ScrollKeyAction::Bottom) => self.scroll_ask_ai_output_to_bottom(),
            None => {}
        }
    }

    fn handle_ask_ai_output_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            app.scroll_ask_ai_output_by(direction, MOUSE_WHEEL_STEP);
        });
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

    fn toggle_help_overlay(&mut self) {
        self.overlay = match self.overlay {
            Some(Overlay::Help { .. }) => None,
            _ => Some(Overlay::Help { scroll: 0 }),
        };
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

    fn scroll_help_overlay_by(&mut self, direction: VerticalDirection, amount: usize) {
        if let Some(Overlay::Help { scroll }) = &mut self.overlay {
            *scroll = direction.shift(*scroll, amount);
        }
    }

    fn command_output_page(&self) -> usize {
        self.command_output()
            .map_or(1, |output| output.visible_height.max(1))
    }

    fn ask_ai_output_page(&self) -> usize {
        self.ask_ai_output()
            .map_or(1, |output| output.visible_height.max(1))
    }

    fn scroll_command_output_by(&mut self, direction: VerticalDirection, amount: usize) {
        let Some(output) = self.command_output_mut() else {
            return;
        };

        output.scroll = direction.shift(output.scroll, amount);
        clamp_command_output_scroll(output);
    }

    fn scroll_command_output_to_top(&mut self) {
        if let Some(output) = self.command_output_mut() {
            output.scroll = 0;
        }
    }

    fn scroll_command_output_to_bottom(&mut self) {
        if let Some(output) = self.command_output_mut() {
            output.scroll = usize::MAX;
            clamp_command_output_scroll(output);
        }
    }

    fn scroll_ask_ai_output_by(&mut self, direction: VerticalDirection, amount: usize) {
        let Some(output) = self.ask_ai_output_mut() else {
            return;
        };

        output.scroll = direction.shift(output.scroll, amount);
        clamp_ask_ai_output_scroll(output);
    }

    fn scroll_ask_ai_output_to_top(&mut self) {
        if let Some(output) = self.ask_ai_output_mut() {
            output.scroll = 0;
        }
    }

    fn scroll_ask_ai_output_to_bottom(&mut self) {
        if let Some(output) = self.ask_ai_output_mut() {
            output.scroll = usize::MAX;
            clamp_ask_ai_output_scroll(output);
        }
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

pub(crate) fn accepts_text_input(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn closes_help_overlay(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || matches!(key.code, KeyCode::Char('?') | KeyCode::Char('q') if accepts_text_input(key))
}

fn closes_command_output(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q') if accepts_text_input(key))
}

fn closes_ask_ai_running(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q') if accepts_text_input(key))
}

fn closes_ask_ai_output(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q') if accepts_text_input(key))
}

fn scroll_key_action(key: KeyEvent) -> Option<ScrollKeyAction> {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => Some(ScrollKeyAction::Line(VerticalDirection::Down)),
        KeyCode::Up | KeyCode::Char('k') => Some(ScrollKeyAction::Line(VerticalDirection::Up)),
        KeyCode::PageDown => Some(ScrollKeyAction::Page(VerticalDirection::Down)),
        KeyCode::PageUp => Some(ScrollKeyAction::Page(VerticalDirection::Up)),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ScrollKeyAction::Page(VerticalDirection::Down))
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ScrollKeyAction::Page(VerticalDirection::Up))
        }
        KeyCode::Home | KeyCode::Char('g') => Some(ScrollKeyAction::Top),
        KeyCode::End | KeyCode::Char('G') => Some(ScrollKeyAction::Bottom),
        _ => None,
    }
}

fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
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

fn clamp_command_output_scroll(output: &mut CommandOutputState) {
    output.scroll = output.scroll.min(
        output
            .rendered_row_count
            .saturating_sub(output.visible_height),
    );
}

fn clamp_ask_ai_output_scroll(output: &mut AskAiOutputState) {
    output.scroll = output.scroll.min(
        output
            .rendered_row_count
            .saturating_sub(output.visible_height),
    );
}

#[cfg(test)]
fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
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

fn initial_selected_hunk_index(changeset: &Changeset) -> Option<usize> {
    changeset
        .files
        .first()
        .and_then(|file| bounded_hunk_index(file, None))
}

fn bounded_hunk_index(file: &DiffFile, index: Option<usize>) -> Option<usize> {
    if file.hunks.is_empty() {
        None
    } else {
        Some(index.unwrap_or(0).min(file.hunks.len() - 1))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask_ai::AskAiReviewMode;
    use crate::model::{DiffHunk, DiffLine, DiffLineKind, FileStatus, SourceSnapshot};
    use crate::theme::Theme;
    use crate::viewport::RenderedDiffLines;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    #[test]
    fn diff_scroll_bounds_use_rendered_rows_when_available() {
        let mut app = app_with(changeset_with_one_file());
        app.viewport.begin_diff(Rect::default(), 3);
        app.diff_scroll = 99;
        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                24,
                Theme::github_dark().syntax,
                true,
                vec![Line::raw("row"); 8],
                true,
            ),
        );

        app.ensure_scroll_bounds();

        assert_eq!(app.diff_scroll, 5);
    }

    #[test]
    fn reload_preserves_selected_file_and_scroll_by_path() {
        let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.viewport.begin_diff(Rect::default(), 3);
        app.diff_scroll = 4;

        app.apply_reloaded_changeset(changeset_with_paths(["b.txt", "a.txt"]), true);

        assert_eq!(
            app.selected_file().map(DiffFile::display_path),
            Some("b.txt")
        );
        assert_eq!(app.selected_file_index, 0);
        assert_eq!(app.diff_scroll, 4);
    }

    #[test]
    fn reload_preserves_selected_hunk_by_coordinates() {
        let mut app = app_with(changeset_with_two_hunk_file());
        app.selected_hunk_index = Some(1);

        app.apply_reloaded_changeset(changeset_with_two_hunk_file(), true);

        assert_eq!(app.selected_hunk_index, Some(1));
    }

    #[test]
    fn reload_clamps_scroll_when_selected_file_shrinks() {
        let mut app = app_with(changeset_with_paths(["sample.txt"]));
        app.viewport.begin_diff(Rect::default(), 3);
        app.diff_scroll = 99;

        app.apply_reloaded_changeset(changeset_with_short_file("sample.txt"), true);

        assert_eq!(
            app.selected_file().map(DiffFile::display_path),
            Some("sample.txt")
        );
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn reload_resets_selection_and_scroll_when_selected_file_disappears() {
        let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.diff_scroll = 4;

        app.apply_reloaded_changeset(changeset_with_paths(["a.txt"]), true);

        assert_eq!(
            app.selected_file().map(DiffFile::display_path),
            Some("a.txt")
        );
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn hiding_files_panel_moves_focus_to_diff() {
        let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.sidebar_scroll = 1;
        app.diff_scroll = 3;

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        assert!(!app.files_panel_visible);
        assert_eq!(app.focus, FocusPane::Diff);
        assert_eq!(app.selected_file_index, 1);
        assert_eq!(app.sidebar_scroll, 1);
        assert_eq!(app.diff_scroll, 3);
    }

    #[test]
    fn hidden_files_panel_cannot_receive_keyboard_focus() {
        let mut app = app_with(changeset_with_one_file());
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focus, FocusPane::Diff);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focus, FocusPane::Diff);
    }

    #[test]
    fn showing_files_panel_moves_focus_to_sidebar() {
        let mut app = app_with(changeset_with_one_file());
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        assert!(app.files_panel_visible);
        assert_eq!(app.focus, FocusPane::Sidebar);
    }

    #[test]
    fn question_mark_toggles_help_overlay() {
        let mut app = app_with(changeset_with_one_file());

        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();
        assert!(app.help_overlay_visible());

        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();
        assert!(!app.help_overlay_visible());
    }

    #[test]
    fn help_overlay_dismisses_without_exiting() {
        let mut app = app_with(changeset_with_one_file());

        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();
        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(keep_running);
        assert!(!app.help_overlay_visible());

        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();
        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert!(keep_running);
        assert!(!app.help_overlay_visible());
    }

    #[test]
    fn ctrl_c_exits_tui() {
        let mut app = app_with(changeset_with_one_file());

        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();

        assert!(!keep_running);
    }

    #[test]
    fn hunk_jump_uses_cached_wrapped_offsets() {
        let mut app = app_with(changeset_with_two_hunk_file());
        let theme = Theme::github_dark();
        app.viewport.begin_diff(Rect::default(), 3);
        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                24,
                theme.syntax,
                true,
                vec![Line::raw("row"); 10],
                false,
            )
            .with_hunk_offsets(vec![1, 80]),
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.diff_scroll, 79);
        assert_eq!(app.selected_hunk_index, Some(1));

        app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.diff_scroll, 0);
        assert_eq!(app.selected_hunk_index, Some(0));
    }

    #[test]
    fn hunk_jump_handles_missing_and_single_offsets() {
        let mut app = app_with(changeset_with_one_file());
        let theme = Theme::github_dark();
        app.viewport.begin_diff(Rect::default(), 3);
        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                24,
                theme.syntax,
                true,
                vec![Line::raw("row"); 8],
                true,
            )
            .with_hunk_offsets(Vec::new()),
        );
        app.diff_scroll = 4;

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.diff_scroll, 4);

        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                24,
                theme.syntax,
                true,
                vec![Line::raw("row"); 8],
                true,
            )
            .with_hunk_offsets(vec![5]),
        );
        app.diff_scroll = 0;

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.diff_scroll, 4);

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.diff_scroll, 4);
    }

    #[test]
    fn scrolling_diff_selects_hunk_at_top_visible_row() {
        let mut app = app_with(changeset_with_two_hunk_file());
        let theme = Theme::github_dark();
        app.viewport.begin_diff(Rect::default(), 3);
        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                24,
                theme.syntax,
                true,
                vec![Line::raw("row"); 100],
                true,
            )
            .with_hunk_offsets(vec![1, 80]),
        );

        app.scroll_diff_by(VerticalDirection::Down, 80);

        assert_eq!(app.selected_hunk_index, Some(1));
    }

    #[test]
    fn selected_hunk_style_is_applied_to_visible_cached_rows() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_two_hunk_file());
        app.selected_hunk_index = Some(1);
        app.diff_scroll = 8;

        let pane = render_diff_pane(&mut app, theme);

        assert!(
            pane.lines
                .iter()
                .any(|line| line_text(line).starts_with("> @@ -20 +20 @@"))
        );
    }

    #[test]
    fn diff_click_selects_hunk_under_pointer() {
        let mut app = app_with(changeset_with_two_hunk_file());
        let theme = Theme::github_dark();
        app.viewport.begin_diff(Rect::new(0, 0, 80, 10), 8);
        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                80,
                theme.syntax,
                true,
                vec![Line::raw("row"); 12],
                true,
            )
            .with_hunk_offsets(vec![1, 5]),
        );

        app.handle_left_down(1, 6);
        app.handle_left_up(1, 6);

        assert_eq!(app.focus, FocusPane::Diff);
        assert_eq!(app.selected_hunk_index, Some(1));
        assert_eq!(app.diff_scroll, 1);
    }

    #[test]
    fn text_drag_requests_clipboard_copy() {
        let mut app = app_with(changeset_with_one_file());
        app.begin_render_frame();
        app.selectable_lines(
            Rect::new(2, 3, 10, 1),
            vec![Line::raw("abcdef")],
            0,
            1,
            Theme::github_dark(),
        );

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 5,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 5,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.take_clipboard_request().as_deref(), Some("bcd"));
    }

    #[test]
    fn diff_scrollbar_click_and_drag_update_scroll() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_file(diff_file("sample.txt", 40)));
        let pane = render_diff_pane(&mut app, theme);
        let scrollbar = pane.scrollbar.expect("large diff should show scrollbar");
        let area = scrollbar.area();

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y + area.height - 1,
            modifiers: KeyModifiers::NONE,
        });

        let clicked_scroll = app.diff_scroll;
        assert_eq!(app.focus, FocusPane::Diff);
        assert!(clicked_scroll > 0);

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        });

        assert!(app.diff_scroll < clicked_scroll);
    }

    #[test]
    fn diff_space_without_hunks_sets_live_error_without_exiting() {
        let mut app = app_with(changeset_with_one_file());
        app.focus = FocusPane::Diff;
        app.changeset.files[0].hunks.clear();

        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            .unwrap();

        assert!(keep_running);
        assert_eq!(app.live_error.as_deref(), Some("no selected hunk to stage"));
    }

    #[test]
    fn discard_key_requires_confirmation() {
        let mut app = app_with(changeset_with_one_file());
        app.focus = FocusPane::Sidebar;

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(
            app.discard_target(),
            Some(DiscardTarget::File { path, .. }) if path == "sample.txt"
        ));
        assert!(app.live_error.is_none());

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();

        assert!(app.discard_target().is_none());
    }

    #[test]
    fn search_prompt_applies_query_scrolls_to_first_match_and_highlights_it() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_file(diff_file_with_contents([
            "alpha",
            "target one",
            "beta",
            "target two",
        ])));

        enter_search_query(&mut app, "target");
        let pane = render_diff_pane(&mut app, theme);

        assert_eq!(app.search.match_count(), 2);
        assert_eq!(app.search.active_index(), Some(0));
        let active_row = app.search.active_match_row().unwrap();
        assert!(active_row >= app.diff_scroll);
        assert!(active_row < app.diff_scroll + app.viewport.diff_view_height());
        assert!(pane.lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content.as_ref() == "target" && span.style.bg == Some(theme.accent)
            })
        }));
    }

    #[test]
    fn search_next_and_previous_cycle_matches() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_file(diff_file_with_contents([
            "target one",
            "middle",
            "target two",
        ])));
        enter_search_query(&mut app, "target");
        render_diff_pane(&mut app, theme);

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.search.active_index(), Some(1));

        app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.search.active_index(), Some(0));
    }

    #[test]
    fn esc_clears_active_search_without_exiting() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_file(diff_file_with_contents([
            "target one",
            "middle",
            "target two",
        ])));
        enter_search_query(&mut app, "target");
        render_diff_pane(&mut app, theme);

        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();

        assert!(keep_running);
        assert!(app.search.active_query().is_none());
        assert_eq!(app.search.match_count(), 0);
        assert_eq!(app.search.active_index(), None);
    }

    #[test]
    fn esc_in_search_prompt_clears_previous_search() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_file(diff_file_with_contents(["target"])));
        enter_search_query(&mut app, "target");
        render_diff_pane(&mut app, theme);

        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .unwrap();
        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();

        assert!(keep_running);
        assert!(!app.search.is_prompt_open());
        assert!(app.search.active_query().is_none());
        assert_eq!(app.search.match_count(), 0);
    }

    #[test]
    fn ctrl_c_exits_from_search_prompt() {
        let mut app = app_with(changeset_with_one_file());
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .unwrap();

        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();

        assert!(!keep_running);
    }

    #[test]
    fn search_no_match_state_is_rendered() {
        let theme = Theme::github_dark();
        let mut app = app_with(changeset_with_file(diff_file_with_contents([
            "alpha", "beta",
        ])));

        enter_search_query(&mut app, "missing");
        let pane = render_diff_pane(&mut app, theme);

        assert!(
            pane.lines
                .iter()
                .any(|line| line_text(line).contains("no matches"))
        );
    }

    #[test]
    fn custom_command_key_queues_command_request() {
        let mut app = app_with_config(AppConfig {
            commands: vec![custom_command("C", "commit", "git commit")],
        });

        app.handle_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT))
            .unwrap();

        let request = app
            .take_custom_command_request()
            .expect("custom command should be queued");
        assert_eq!(request.label(), "commit");
        assert_eq!(request.command(), "git commit");
    }

    #[test]
    fn custom_commands_are_help_only_not_footer_hints() {
        let app = app_with_config(AppConfig {
            commands: vec![custom_command("P", "publish", "git push")],
        });
        let theme = Theme::github_dark();

        let footer = line_text(&app.keybind_bar_line(theme));
        let help = app
            .help_overlay_lines(80, theme)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!footer.contains("publish"), "footer was {footer:?}");
        assert!(help.contains("Custom commands"));
        assert!(help.contains("P publish  git push"));
    }

    #[test]
    fn footer_keeps_secondary_actions_in_help_only() {
        let app = app_with(changeset_with_one_file());
        let theme = Theme::github_dark();

        let footer = line_text(&app.keybind_bar_line(theme));
        let help = app
            .help_overlay_lines(80, theme)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!footer.contains("discard"), "footer was {footer:?}");
        assert!(!footer.contains("ask AI"), "footer was {footer:?}");
        assert!(!footer.contains("explain"), "footer was {footer:?}");
        assert!(help.contains("d discard focused file or hunk"));
        assert!(help.contains("a Ask AI about focused file or hunk"));
        assert!(help.contains("x Explain focused file or hunk with Ask AI"));
    }

    #[test]
    fn custom_command_running_indicator_is_replaced_by_output() {
        let mut app = app_with(changeset_with_one_file());
        let command = custom_command("C", "commit and push", "git commit && git push");

        app.set_custom_command_running(&command);
        let running_pane = render_diff_pane(&mut app, Theme::github_dark());
        let running_text = pane_text(&running_pane);

        assert!(running_text.contains("⠋ Running command: commit and push"));

        app.advance_custom_command_spinner();
        let next_running_pane = render_diff_pane(&mut app, Theme::github_dark());
        let next_running_text = pane_text(&next_running_pane);

        assert!(next_running_text.contains("⠙ Running command: commit and push"));

        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();

        assert!(keep_running);
        assert!(app.command_running().is_some());

        app.set_custom_command_result(CustomCommandResult::not_started(&command, None, "failed"));
        let output_pane = render_diff_pane(&mut app, Theme::github_dark());
        let output_text = pane_text(&output_pane);

        assert!(app.command_running().is_none());
        assert!(output_pane.title.contains("Command: commit and push"));
        assert!(!output_text.contains("Running command: commit and push"));
    }

    #[test]
    fn command_output_pane_scrolls_and_closes() {
        let mut app = app_with(changeset_with_one_file());
        let command = custom_command("C", "long output", "false");
        app.set_custom_command_result(CustomCommandResult::not_started(
            &command,
            None,
            "one\ntwo\nthree\nfour\nfive\nsix\nseven",
        ));

        let pane = render_diff_pane(&mut app, Theme::github_dark());
        assert!(pane.title.contains("Command: long output"));
        assert_eq!(app.command_output().map(|output| output.scroll), Some(0));

        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.command_output().map(|output| output.scroll), Some(1));

        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert!(app.command_output().is_none());
    }

    #[test]
    fn ask_ai_key_from_files_panel_queues_file_context() {
        let mut changeset = changeset_with_one_file();
        changeset.title = "Tracked changes".to_string();
        changeset.source_label = "git diff HEAD + untracked".to_string();
        let mut app = app_with(changeset);

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        let prompt_pane = render_diff_pane(&mut app, Theme::github_dark());

        assert!(pane_text(&prompt_pane).contains("Ask AI: type a question"));
        assert!(matches!(app.overlay, Some(Overlay::AskAiPrompt(_))));

        enter_ask_ai_question(&mut app, "Why changed?");

        let request = app
            .take_ask_ai_request()
            .expect("Ask AI request should be queued");
        assert_eq!(request.question(), "Why changed?");
        assert_eq!(request.context().summary(), "sample.txt");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn ask_ai_key_from_diff_pane_queues_hunk_context() {
        let mut changeset = changeset_with_one_file();
        changeset.title = "Tracked changes".to_string();
        changeset.source_label = "git diff HEAD + untracked".to_string();
        let mut app = app_with(changeset);
        app.focus = FocusPane::Diff;

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        let prompt_pane = render_diff_pane(&mut app, Theme::github_dark());

        assert!(pane_text(&prompt_pane).contains("Ask AI: type a question"));
        assert!(matches!(app.overlay, Some(Overlay::AskAiPrompt(_))));

        enter_ask_ai_question(&mut app, "Why changed?");

        let request = app
            .take_ask_ai_request()
            .expect("Ask AI request should be queued");
        assert_eq!(request.question(), "Why changed?");
        assert_eq!(request.context().summary(), "sample.txt hunk 1");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn explain_code_key_from_files_panel_queues_file_context() {
        let mut changeset = changeset_with_one_file();
        changeset.title = "Tracked changes".to_string();
        changeset.source_label = "git diff HEAD + untracked".to_string();
        let mut app = app_with(changeset);

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();

        let request = app
            .take_ask_ai_request()
            .expect("Explain Code request should be queued");
        assert_explain_code_question(request.question());
        assert_eq!(request.context().summary(), "sample.txt");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn explain_code_key_from_diff_pane_queues_hunk_context() {
        let mut changeset = changeset_with_one_file();
        changeset.title = "Tracked changes".to_string();
        changeset.source_label = "git diff HEAD + untracked".to_string();
        let mut app = app_with(changeset);
        app.focus = FocusPane::Diff;

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();

        let request = app
            .take_ask_ai_request()
            .expect("Explain Code request should be queued");
        assert_explain_code_question(request.question());
        assert_eq!(request.context().summary(), "sample.txt hunk 1");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn ask_ai_running_can_be_cancelled() {
        let mut app = app_with(changeset_with_one_file());
        let request = ask_ai_request("Explain this");

        app.set_ask_ai_running(&request);
        let running_pane = render_diff_pane(&mut app, Theme::github_dark());
        assert!(pane_text(&running_pane).contains("Asking AI: Explain this"));

        let keep_running = app
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();

        assert!(keep_running);
        assert!(matches!(
            app.overlay,
            Some(Overlay::AskAiRunning {
                cancelling: true,
                ..
            })
        ));
        assert!(app.take_ask_ai_cancel_request());
        assert!(!app.take_ask_ai_cancel_request());
    }

    #[test]
    fn ask_ai_output_pane_scrolls_and_closes() {
        let mut app = app_with(changeset_with_one_file());
        let request = ask_ai_request("Explain this");
        app.set_ask_ai_result(AskAiResult::not_started(
            request,
            None,
            "one\ntwo\nthree\nfour\nfive\nsix\nseven",
        ));

        let pane = render_diff_pane(&mut app, Theme::github_dark());
        assert!(pane.title.contains("Ask AI: sample.txt hunk 1"));
        assert!(pane_text(&pane).contains("question: Explain this"));
        assert_eq!(app.ask_ai_output().map(|output| output.scroll), Some(0));

        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.ask_ai_output().map(|output| output.scroll), Some(1));

        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert!(app.ask_ai_output().is_none());
    }

    fn app_with(changeset: Changeset) -> App {
        App::new(LoadedReview::worktree(changeset))
    }

    fn app_with_config(config: AppConfig) -> App {
        App::with_config(LoadedReview::worktree(changeset_with_one_file()), config)
    }

    fn custom_command(key: &str, label: &str, command: &str) -> CustomCommandBinding {
        CustomCommandBinding::new(
            crate::custom_command::CommandKey::parse(key).unwrap(),
            label.to_string(),
            command.to_string(),
        )
    }

    fn enter_search_query(app: &mut App, query: &str) {
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .unwrap();
        for character in query.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
    }

    fn enter_ask_ai_question(app: &mut App, question: &str) {
        for character in question.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
    }

    fn ask_ai_request(question: &str) -> AskAiRequest {
        let file = diff_file("sample.txt", 1);
        let context = AskAiContext::focused(
            AskAiReviewMode::Worktree,
            "Tracked changes".to_string(),
            "git diff HEAD + untracked".to_string(),
            &file,
            Some(0),
            None,
        );

        AskAiRequest::new(question.to_string(), context)
    }

    fn assert_explain_code_question(question: &str) {
        assert!(question.contains("Explain the selected or focused code"));
        assert!(question.contains("what the code does"));
        assert!(question.contains("why the changed code matters"));
        assert!(question.contains("assumptions or risks"));
        assert!(question.contains("read-only"));
    }

    fn render_diff_pane(app: &mut App, theme: Theme) -> DiffPaneRows {
        app.diff_pane_rows(Rect::new(0, 0, 80, 8), 80, 6, theme)
    }

    fn pane_text(pane: &DiffPaneRows) -> String {
        pane.lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn changeset_with_file(file: DiffFile) -> Changeset {
        Changeset {
            title: String::new(),
            source_label: String::new(),
            files: vec![file],
        }
    }

    fn changeset_with_one_file() -> Changeset {
        changeset_with_paths(["sample.txt"])
    }

    fn changeset_with_two_hunk_file() -> Changeset {
        let mut changeset = changeset_with_one_file();
        changeset.files[0].hunks.push(DiffHunk {
            header: "@@ -20 +20 @@".to_string(),
            old_start: 20,
            old_lines: 1,
            new_start: 20,
            new_lines: 1,
            stage: crate::model::FileStage::Unstaged,
            lines: vec![DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(20),
                new_line: Some(20),
                content: "line".to_string(),
            }],
        });
        changeset
    }

    fn changeset_with_short_file(path: &str) -> Changeset {
        changeset_with_file(diff_file(path, 1))
    }

    fn changeset_with_paths<const N: usize>(paths: [&str; N]) -> Changeset {
        Changeset {
            title: String::new(),
            source_label: String::new(),
            files: paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    let mut file = diff_file(path, 8);
                    file.id = index.to_string();
                    file
                })
                .collect(),
        }
    }

    fn diff_file(path: &str, line_count: u32) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: crate::model::FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: format!("@@ -1,{line_count} +1,{line_count} @@"),
                old_start: 1,
                old_lines: line_count,
                new_start: 1,
                new_lines: line_count,
                stage: crate::model::FileStage::Unstaged,
                lines: (1..=line_count)
                    .map(|line_number| DiffLine {
                        kind: DiffLineKind::Context,
                        old_line: Some(line_number),
                        new_line: Some(line_number),
                        content: "line".to_string(),
                    })
                    .collect(),
            }],
            binary: false,
        }
    }

    fn diff_file_with_contents<const N: usize>(contents: [&str; N]) -> DiffFile {
        let line_count = contents.len() as u32;
        DiffFile {
            id: "0".to_string(),
            old_path: "sample.txt".to_string(),
            path: "sample.txt".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: crate::model::FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: format!("@@ -1,{line_count} +1,{line_count} @@"),
                old_start: 1,
                old_lines: line_count,
                new_start: 1,
                new_lines: line_count,
                stage: crate::model::FileStage::Unstaged,
                lines: contents
                    .into_iter()
                    .enumerate()
                    .map(|(index, content)| {
                        let line_number = index as u32 + 1;
                        DiffLine {
                            kind: DiffLineKind::Context,
                            old_line: Some(line_number),
                            new_line: Some(line_number),
                            content: content.to_string(),
                        }
                    })
                    .collect(),
            }],
            binary: false,
        }
    }
}
