use ratatui::text::Line;

use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::rows;
use crate::scroll_text::VerticalDirection;
use crate::search::Search;
use crate::theme::Theme;
use crate::viewport::{RenderedViewport, ViewportScrollInput};

#[derive(Debug, Default)]
pub(super) struct DiffPaneState {
    scroll: usize,
    selected_hunk_index: Option<usize>,
    search: Search,
}

impl DiffPaneState {
    pub(super) fn new(changeset: &Changeset) -> Self {
        Self {
            scroll: 0,
            selected_hunk_index: initial_selected_hunk_index(changeset),
            search: Search::default(),
        }
    }

    pub(super) fn scroll(&self) -> usize {
        self.scroll
    }

    pub(super) fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    pub(super) fn selected_hunk_index(&self) -> Option<usize> {
        self.selected_hunk_index
    }

    pub(super) fn set_selected_hunk_index(&mut self, index: Option<usize>) {
        self.selected_hunk_index = index;
    }

    pub(super) fn selected_hunk<'a>(&self, file: Option<&'a DiffFile>) -> Option<&'a DiffHunk> {
        file?.hunks.get(self.selected_hunk_index?)
    }

    pub(super) fn select_file(&mut self, file: Option<&DiffFile>) {
        self.selected_hunk_index = file.and_then(|file| bounded_hunk_index(file, None));
        self.scroll = 0;
        self.invalidate_search_matches();
    }

    pub(super) fn ensure_bounds(
        &mut self,
        file: Option<&DiffFile>,
        viewport: &RenderedViewport,
        input: ViewportScrollInput<'_>,
    ) -> usize {
        self.ensure_selected_hunk_bounds(file);
        let scrolls = viewport.clamped_scrolls(input);
        self.scroll = scrolls.diff_scroll;
        scrolls.sidebar_scroll
    }

    pub(super) fn scroll_page(
        &mut self,
        direction: VerticalDirection,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.scroll_by(
            direction,
            viewport.diff_view_height(),
            viewport,
            selected_file_index,
            file,
        );
    }

    pub(super) fn scroll_by(
        &mut self,
        direction: VerticalDirection,
        amount: usize,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.scroll = direction.shift(self.scroll, amount);
        self.select_hunk_at_scroll(viewport, selected_file_index, file);
    }

    pub(super) fn scroll_to(
        &mut self,
        scroll: usize,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.scroll = scroll;
        self.select_hunk_at_scroll(viewport, selected_file_index, file);
    }

    pub(super) fn scroll_to_top(
        &mut self,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.scroll_to(0, viewport, selected_file_index, file);
    }

    pub(super) fn scroll_to_bottom(&mut self, file: Option<&DiffFile>) {
        self.scroll = usize::MAX;
        self.selected_hunk_index = file.and_then(|file| file.hunks.len().checked_sub(1));
    }

    pub(super) fn jump_by(
        &mut self,
        direction: VerticalDirection,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        if self.has_active_search() {
            self.jump_search_match(direction, viewport, selected_file_index, file);
        } else {
            self.jump_hunk(direction, viewport, selected_file_index, file);
        }
    }

    pub(super) fn center_selected_hunk(
        &mut self,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        let Some(index) = self.selected_hunk_index else {
            return;
        };
        let Some(file) = file else {
            return;
        };
        if index >= file.hunks.len() {
            return;
        }

        if let Some(offset) = viewport.hunk_offset(selected_file_index, file.id.as_str(), index) {
            self.scroll = offset.saturating_sub(viewport.diff_view_height() / 2);
        }
    }

    pub(super) fn select_hunk_at_scroll(
        &mut self,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.selected_hunk_index =
            self.hunk_index_at_rendered_row(viewport, selected_file_index, file, self.scroll);
    }

    pub(super) fn hunk_index_at_rendered_row(
        &self,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
        rendered_row: usize,
    ) -> Option<usize> {
        let file = file?;
        viewport.hunk_index_at(
            selected_file_index,
            file.id.as_str(),
            rendered_row,
            file.hunks.len(),
        )
    }

    pub(super) fn search_prompt_open(&self) -> bool {
        self.search.is_prompt_open()
    }

    pub(super) fn has_active_search(&self) -> bool {
        self.search.active_query().is_some()
    }

    pub(super) fn search_status(&self) -> Option<rows::SearchStatus<'_>> {
        self.search.status()
    }

    pub(super) fn open_search_prompt(&mut self) {
        self.search.open_prompt();
    }

    pub(super) fn handle_search_prompt_key(&mut self, key: crossterm::event::KeyEvent) {
        self.search.handle_prompt_key(key);
    }

    pub(super) fn clear_search_query(&mut self) {
        self.search.clear_query();
    }

    pub(super) fn clear_rendered_search_matches(&mut self) {
        self.search.clear_rendered_matches();
    }

    pub(super) fn invalidate_search_matches(&mut self) {
        self.search.invalidate_matches();
    }

    pub(super) fn render_target_rows(&self, visible_height: usize) -> usize {
        if self.has_active_search() {
            return usize::MAX;
        }

        self.scroll
            .saturating_add(visible_height)
            .saturating_add(rows::DIFF_PREFETCH_ROWS)
    }

    pub(super) fn refresh_search_matches(
        &mut self,
        files: &[DiffFile],
        viewport: &RenderedViewport,
        selected_file_index: usize,
    ) -> bool {
        let Some(file_id) = files.get(selected_file_index).map(|file| file.id.as_str()) else {
            self.clear_rendered_search_matches();
            return false;
        };

        let Some(lines) = viewport.diff_lines(selected_file_index, file_id) else {
            self.clear_rendered_search_matches();
            return false;
        };

        self.search.refresh_matches(file_id, lines)
    }

    pub(super) fn highlight_search_matches(
        &self,
        lines: Vec<Line<'static>>,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.search.highlight(lines, self.scroll, theme)
    }

    pub(super) fn scroll_active_search_match(
        &mut self,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        let Some(active_row) = self.search.active_match_row() else {
            return;
        };

        self.scroll = active_row.saturating_sub(viewport.diff_view_height() / 2);
        self.select_hunk_at_scroll(viewport, selected_file_index, file);
    }

    #[cfg(test)]
    pub(super) fn search_match_count(&self) -> usize {
        self.search.match_count()
    }

    #[cfg(test)]
    pub(super) fn active_search_index(&self) -> Option<usize> {
        self.search.active_index()
    }

    #[cfg(test)]
    pub(super) fn active_search_match_row(&self) -> Option<usize> {
        self.search.active_match_row()
    }

    #[cfg(test)]
    pub(super) fn active_search_query(&self) -> Option<&str> {
        self.search.active_query()
    }

    fn ensure_selected_hunk_bounds(&mut self, file: Option<&DiffFile>) {
        let Some(file) = file else {
            self.selected_hunk_index = None;
            return;
        };

        self.selected_hunk_index = bounded_hunk_index(file, self.selected_hunk_index);
    }

    fn jump_hunk(
        &mut self,
        direction: VerticalDirection,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        let Some(file) = file else {
            return;
        };
        if file.hunks.is_empty() {
            self.selected_hunk_index = None;
            return;
        }

        let current = self
            .selected_hunk_index
            .or_else(|| {
                self.hunk_index_at_rendered_row(
                    viewport,
                    selected_file_index,
                    Some(file),
                    self.scroll,
                )
            })
            .unwrap_or(0)
            .min(file.hunks.len() - 1);
        let target = direction.shift_clamped(current, 1, file.hunks.len() - 1);

        self.selected_hunk_index = Some(target);
        self.center_selected_hunk(viewport, selected_file_index, Some(file));
    }

    fn jump_search_match(
        &mut self,
        direction: VerticalDirection,
        viewport: &RenderedViewport,
        selected_file_index: usize,
        file: Option<&DiffFile>,
    ) {
        if self
            .search
            .advance_match(matches!(direction, VerticalDirection::Down))
        {
            self.scroll_active_search_match(viewport, selected_file_index, file);
        }
    }
}

pub(super) fn initial_selected_hunk_index(changeset: &Changeset) -> Option<usize> {
    changeset
        .files
        .first()
        .and_then(|file| bounded_hunk_index(file, None))
}

pub(super) fn bounded_hunk_index(file: &DiffFile, index: Option<usize>) -> Option<usize> {
    if file.hunks.is_empty() {
        None
    } else {
        Some(index.unwrap_or(0).min(file.hunks.len() - 1))
    }
}
