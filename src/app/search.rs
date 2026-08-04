use crossterm::event::KeyEvent;

use crate::scroll_text::VerticalDirection;

use super::{App, FocusPane};

impl App {
    pub(super) fn search_prompt_open(&self) -> bool {
        self.selected_diff.search_prompt_open()
    }

    pub(super) fn open_search_prompt(&mut self) {
        self.focus = FocusPane::Diff;
        self.selected_diff.open_search_prompt();
    }

    pub(super) fn handle_search_prompt_key(&mut self, key: KeyEvent) {
        self.selected_diff.handle_search_prompt_key(key);
        self.ensure_scroll_bounds();
    }

    pub(super) fn clear_search_query(&mut self) {
        self.selected_diff.clear_search_query();
    }

    pub(super) fn jump_by(&mut self, direction: VerticalDirection) {
        let selected_file_index = self.selected_file_index();
        self.selected_diff.jump_by(
            direction,
            selected_file_index,
            self.changeset.files.get(selected_file_index),
        );
    }
}
