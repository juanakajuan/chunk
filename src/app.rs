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

use crate::editor::EditorRequest;
use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::review_source::{LoadedReview, ReviewSource};
use crate::rows::{self, SidebarRowsInput};
use crate::theme::Theme;
use crate::viewport::{
    DiffRenderRequest, RenderedDiffLines, RenderedViewport, ViewportScrollInput,
};

const MOUSE_WHEEL_STEP: usize = 3;

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
    /// First rendered diff row visible in the diff pane.
    diff_scroll: usize,
    /// Rendered live-status rows above the diff rows in the diff pane.
    diff_status_rows: usize,
    /// First file index considered for sidebar rendering.
    sidebar_scroll: usize,
    /// Rendered viewport geometry, row mapping, and render caches.
    viewport: RenderedViewport,
    /// Deferred request for runtime to open an external editor safely.
    editor_request: Option<EditorRequest>,
}

pub(crate) struct DiffPaneRows {
    pub(crate) title: String,
    pub(crate) lines: Vec<Line<'static>>,
}

impl App {
    pub(crate) fn new(review: LoadedReview) -> Self {
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
            diff_scroll: 0,
            diff_status_rows: 0,
            sidebar_scroll: 0,
            viewport: RenderedViewport::new(file_count),
            editor_request: None,
        }
    }

    pub(crate) fn begin_render_frame(&mut self) {
        self.viewport.begin_frame();
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

        rendered_rows.lines
    }

    pub(crate) fn diff_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        let title = format!(" {} ", rows::changeset_title(&self.changeset));
        let mut lines = rows::live_status_lines(self.live_error.as_deref(), content_width, theme);
        self.diff_status_rows = lines.len();
        let visible_diff_height = visible_height.saturating_sub(lines.len());
        self.viewport.begin_diff(area, visible_diff_height);
        self.ensure_scroll_bounds();

        if visible_diff_height > 0 {
            lines.extend(self.selected_diff_lines(content_width, visible_diff_height, theme));
        }
        lines.truncate(visible_height);

        DiffPaneRows { title, lines }
    }

    pub(crate) fn keybind_bar_line(&self, theme: Theme) -> Line<'static> {
        rows::keybind_bar_line(self.files_panel_visible, self.stage_keybind_hint(), theme)
    }

    fn can_stage(&self) -> bool {
        self.source.can_stage()
    }

    fn stage_keybind_hint(&self) -> Option<&'static str> {
        if !self.can_stage() {
            return None;
        }

        match self.focus {
            FocusPane::Sidebar if self.files_panel_visible => Some("[Space] stage file"),
            FocusPane::Diff => Some("[Space] stage hunk"),
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

    fn ensure_selected_file_sources_loaded(&mut self) {
        let source = &self.source;
        if let Some(file) = self.changeset.files.get_mut(self.selected_file_index) {
            source.load_source_snapshots(file);
        }
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

    fn selected_diff_lines(
        &mut self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.viewport
            .ensure_diff_lines_cache_len(self.changeset.files.len());

        let selected_file_index = self.selected_file_index;
        let can_stage = self.can_stage();
        let selected_hunk_index = self.selected_hunk_index;
        if selected_file_index >= self.changeset.files.len() {
            return rows::no_diff_lines(self.no_diff_message(), content_width, theme);
        }

        let target_rows = self
            .diff_scroll
            .saturating_add(visible_height)
            .saturating_add(rows::DIFF_PREFETCH_ROWS);

        let render_target = {
            let file = &self.changeset.files[selected_file_index];
            self.viewport.diff_lines_render_target(DiffRenderRequest {
                file_index: selected_file_index,
                file_id: file.id.as_str(),
                content_width,
                syntax_palette: theme.syntax,
                can_stage,
                selected_hunk_index,
                requested_rows: target_rows,
            })
        };

        if let Some(render_target) = render_target {
            self.ensure_selected_file_sources_loaded();
            let file = self.changeset.files[selected_file_index].clone();
            let hunk_offsets = rows::hunk_offsets(&file, content_width, theme, can_stage);
            let rendered_rows = rows::diff_lines_until(
                &file,
                content_width,
                theme,
                can_stage,
                selected_hunk_index,
                render_target,
            );
            self.viewport.cache_diff_lines(
                selected_file_index,
                RenderedDiffLines::new(
                    file.id.clone(),
                    content_width,
                    theme.syntax,
                    can_stage,
                    selected_hunk_index,
                    rendered_rows.lines,
                    rendered_rows.complete,
                )
                .with_hunk_offsets(hunk_offsets),
            );
        }

        self.ensure_scroll_bounds();

        self.viewport
            .visible_diff_lines(selected_file_index, self.diff_scroll, visible_height)
    }

    pub(crate) fn set_live_error(&mut self, error: String) {
        self.live_error = Some(error);
    }

    pub(crate) fn take_editor_request(&mut self) -> Option<EditorRequest> {
        self.editor_request.take()
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
        self.selected_file_index = selected_file_index;
        let selected_hunk_index = reloaded_hunk_index(
            self.changeset.files.get(selected_file_index),
            kept_selection,
            previous_hunk_identity,
            previous_hunk_index,
        );
        self.selected_hunk_index = selected_hunk_index;
        self.diff_scroll = if preserve_scroll && kept_selection {
            previous_scroll
        } else {
            0
        };
        self.clear_render_caches();
        self.ensure_scroll_bounds();
    }

    fn clear_render_caches(&mut self) {
        self.viewport
            .clear_render_caches(self.changeset.files.len());
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),

            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Char('f') => self.toggle_files_panel(),
            KeyCode::Left if self.files_panel_visible => self.focus = FocusPane::Sidebar,
            KeyCode::Right | KeyCode::Enter => self.focus = FocusPane::Diff,

            KeyCode::Char('j') => self.move_by(VerticalDirection::Down),
            KeyCode::Char('k') => self.move_by(VerticalDirection::Up),

            KeyCode::Char('n') => self.jump_hunk(VerticalDirection::Down),
            KeyCode::Char('N') => self.jump_hunk(VerticalDirection::Up),

            KeyCode::Home | KeyCode::Char('g') => self.scroll_diff_to_top(),
            KeyCode::End | KeyCode::Char('G') => self.scroll_diff_to_bottom(),

            KeyCode::Char(' ') => self.toggle_selected_staging(),
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
        let column = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_left_click(column, row),
            MouseEventKind::ScrollDown => self.handle_wheel(column, row, VerticalDirection::Down),
            MouseEventKind::ScrollUp => self.handle_wheel(column, row, VerticalDirection::Up),
            MouseEventKind::Moved => self.handle_hover(column, row),
            _ => {}
        }

        self.ensure_scroll_bounds();
    }

    fn handle_left_click(&mut self, column: u16, row: u16) {
        if let Some(index) = self.sidebar_index_at(column, row) {
            self.focus = FocusPane::Sidebar;
            self.select_file(index);
            return;
        }

        if self.is_diff_at(column, row) {
            self.focus = FocusPane::Diff;
            if let Some(index) = self.diff_hunk_index_at(column, row) {
                self.selected_hunk_index = Some(index);
                self.center_selected_hunk();
            }
        }
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
            Some(FocusPane::Sidebar)
        } else if self.is_diff_at(column, row) {
            Some(FocusPane::Diff)
        } else {
            None
        }
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

    fn move_by(&mut self, direction: VerticalDirection) {
        match self.focus {
            FocusPane::Sidebar => self.select_file_by(direction, 1),
            FocusPane::Diff => self.scroll_diff_by(direction, 1),
        }
    }

    fn select_file_by(&mut self, direction: VerticalDirection, amount: usize) {
        let index = match direction {
            VerticalDirection::Down => {
                let max_index = self.changeset.files.len().saturating_sub(1);
                self.selected_file_index
                    .saturating_add(amount)
                    .min(max_index)
            }
            VerticalDirection::Up => self.selected_file_index.saturating_sub(amount),
        };

        self.select_file(index);
    }

    fn select_file(&mut self, index: usize) {
        if self.changeset.files.is_empty() {
            return;
        }

        self.selected_file_index = index.min(self.changeset.files.len() - 1);
        let selected_hunk_index = self
            .selected_file()
            .and_then(|file| bounded_hunk_index(file, None));
        self.selected_hunk_index = selected_hunk_index;
        self.diff_scroll = 0;
    }

    fn scroll_diff_page(&mut self, direction: VerticalDirection) {
        self.scroll_diff_by(direction, self.viewport.diff_view_height());
    }

    fn scroll_diff_by(&mut self, direction: VerticalDirection, amount: usize) {
        self.diff_scroll = match direction {
            VerticalDirection::Down => self.diff_scroll.saturating_add(amount),
            VerticalDirection::Up => self.diff_scroll.saturating_sub(amount),
        };
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
        let target = match direction {
            VerticalDirection::Down => current.saturating_add(1).min(hunk_count - 1),
            VerticalDirection::Up => current.saturating_sub(1),
        };

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

        let Some(offsets) = self
            .viewport
            .diff_hunk_offsets(self.selected_file_index, file_id.as_str())
        else {
            return;
        };

        if let Some(offset) = offsets.get(index) {
            self.diff_scroll = offset.saturating_sub(self.viewport.diff_view_height() / 2);
        }
    }

    fn select_hunk_at_scroll(&mut self) {
        self.selected_hunk_index = self.hunk_index_at_rendered_row(self.diff_scroll);
    }

    fn hunk_index_at_rendered_row(&self, rendered_row: usize) -> Option<usize> {
        let file = self.selected_file()?;
        if file.hunks.is_empty() {
            return None;
        }

        let Some(offsets) = self
            .viewport
            .diff_hunk_offsets(self.selected_file_index, file.id.as_str())
            .filter(|offsets| !offsets.is_empty())
        else {
            return Some(0);
        };

        Some(
            offsets
                .iter()
                .rposition(|offset| *offset <= rendered_row)
                .unwrap_or(0)
                .min(file.hunks.len() - 1),
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

    if kept_file_selection {
        if let Some(index) = previous_identity.and_then(|identity| find_hunk_index(file, identity))
        {
            return Some(index);
        }

        if let Some(index) = previous_index {
            return Some(index.min(file.hunks.len() - 1));
        }
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
                None,
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
                Some(0),
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
                Some(0),
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
                Some(0),
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
                Some(0),
                vec![Line::raw("row"); 100],
                true,
            )
            .with_hunk_offsets(vec![1, 80]),
        );

        app.scroll_diff_by(VerticalDirection::Down, 80);

        assert_eq!(app.selected_hunk_index, Some(1));
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
                Some(0),
                vec![Line::raw("row"); 12],
                true,
            )
            .with_hunk_offsets(vec![1, 5]),
        );

        app.handle_left_click(1, 6);

        assert_eq!(app.focus, FocusPane::Diff);
        assert_eq!(app.selected_hunk_index, Some(1));
        assert_eq!(app.diff_scroll, 1);
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

    fn app_with(changeset: Changeset) -> App {
        App::new(LoadedReview::worktree(changeset))
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
        Changeset {
            title: String::new(),
            source_label: String::new(),
            files: vec![diff_file(path, 1)],
        }
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
}
