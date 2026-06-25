use crossterm::event::KeyEvent;
use ratatui::text::Line;

use crate::rows;
use crate::scroll_text::VerticalDirection;
use crate::theme::Theme;

use super::{App, FocusPane};

impl App {
    pub(super) fn search_prompt_open(&self) -> bool {
        self.diff_pane.search_prompt_open()
    }

    pub(super) fn has_active_search(&self) -> bool {
        self.diff_pane.has_active_search()
    }

    pub(super) fn search_status_lines(
        &self,
        content_width: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        rows::search_status_lines(self.diff_pane.search_status(), content_width, theme)
    }

    pub(super) fn open_search_prompt(&mut self) {
        self.focus = FocusPane::Diff;
        self.diff_pane.open_search_prompt();
    }

    pub(super) fn handle_search_prompt_key(&mut self, key: KeyEvent) {
        self.diff_pane.handle_search_prompt_key(key);
        self.ensure_scroll_bounds();
    }

    pub(super) fn clear_search_query(&mut self) {
        self.diff_pane.clear_search_query();
    }

    pub(super) fn clear_rendered_search_matches(&mut self) {
        self.diff_pane.clear_rendered_search_matches();
    }

    pub(super) fn invalidate_search_matches(&mut self) {
        self.diff_pane.invalidate_search_matches();
    }

    pub(super) fn diff_render_target_rows(&self, visible_height: usize) -> usize {
        self.diff_pane.render_target_rows(visible_height)
    }

    pub(super) fn refresh_search_matches(&mut self, selected_file_index: usize) -> bool {
        self.diff_pane.refresh_search_matches(
            &self.diff_render,
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        )
    }

    pub(super) fn highlight_search_matches(
        &self,
        lines: Vec<Line<'static>>,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.diff_pane.highlight_search_matches(lines, theme)
    }

    pub(super) fn jump_by(&mut self, direction: VerticalDirection) {
        self.diff_pane.jump_by(
            direction,
            &self.viewport,
            &self.diff_render,
            self.selected_file_index,
            self.changeset.files.get(self.selected_file_index),
        );
    }
}
