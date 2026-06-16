use crossterm::event::KeyEvent;
use ratatui::text::Line;

use crate::rows;
use crate::scroll_text::VerticalDirection;
use crate::theme::Theme;

use super::{App, FocusPane};

impl App {
    pub(super) fn search_prompt_open(&self) -> bool {
        self.search.is_prompt_open()
    }

    pub(super) fn has_active_search(&self) -> bool {
        self.search.active_query().is_some()
    }

    pub(super) fn search_status_lines(
        &self,
        content_width: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        rows::search_status_lines(self.search.status(), content_width, theme)
    }

    pub(super) fn open_search_prompt(&mut self) {
        self.focus = FocusPane::Diff;
        self.search.open_prompt();
    }

    pub(super) fn handle_search_prompt_key(&mut self, key: KeyEvent) {
        self.search.handle_prompt_key(key);
        self.ensure_scroll_bounds();
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

    pub(super) fn diff_render_target_rows(&self, visible_height: usize) -> usize {
        if self.has_active_search() {
            return usize::MAX;
        }

        self.diff_scroll
            .saturating_add(visible_height)
            .saturating_add(rows::DIFF_PREFETCH_ROWS)
    }

    pub(super) fn refresh_search_matches(&mut self, selected_file_index: usize) -> bool {
        let Some(file_id) = self
            .changeset
            .files
            .get(selected_file_index)
            .map(|file| file.id.as_str())
        else {
            self.clear_rendered_search_matches();
            return false;
        };

        let Some(lines) = self.viewport.diff_lines(selected_file_index, file_id) else {
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
        self.search.highlight(lines, self.diff_scroll, theme)
    }

    pub(super) fn jump_by(&mut self, direction: VerticalDirection) {
        if self.has_active_search() {
            self.jump_search_match(direction);
        } else {
            self.jump_hunk(direction);
        }
    }

    pub(super) fn scroll_active_search_match(&mut self) {
        let Some(active_row) = self.search.active_match_row() else {
            return;
        };

        self.diff_scroll = active_row.saturating_sub(self.viewport.diff_view_height() / 2);
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
}
