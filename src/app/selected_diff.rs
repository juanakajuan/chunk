//! The selected review diff as one stateful module.
//!
//! This is the ownership boundary for diff-local state: scroll and hunk
//! selection, search, rendered-row caches, frame geometry, hit-testing, and
//! scrollbar capture. `App` supplies the selected review and leading status
//! rows; output panes and text selection deliberately stay outside.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::diff_render::{DiffRenderState, SelectedDiffRenderRequest};
use crate::model::{Changeset, DiffFile, DiffHunk};
use crate::review_source::ReviewSource;
use crate::rows;
use crate::scroll_text::VerticalDirection;
use crate::theme::Theme;
use crate::viewport::{DiffScrollbar, DiffScrollbarDrag, DiffViewport};

use super::diff_pane::DiffPaneState;
use super::saturating_u16;

#[derive(Debug)]
pub(super) struct SelectedDiffPane {
    state: DiffPaneState,
    render: DiffRenderState,
    viewport: DiffViewport,
    scrollbar_drag: Option<DiffScrollbarDrag>,
}

pub(super) struct SelectedReviewFrame<'a> {
    pub(super) area: Rect,
    pub(super) content_width: usize,
    pub(super) visible_height: usize,
    pub(super) theme: Theme,
    pub(super) can_stage: bool,
    pub(super) file_index: usize,
    pub(super) changeset: &'a mut Changeset,
    pub(super) source: &'a ReviewSource,
    pub(super) no_diff_message: &'static str,
    pub(super) leading_rows: Vec<Line<'static>>,
}

pub(super) struct RenderedSelectedDiff {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) scrollbar: Option<DiffScrollbar>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DiffReloadPosition {
    hunk_identity: Option<HunkIdentity>,
    hunk_index: Option<usize>,
    scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HunkIdentity {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

impl SelectedDiffPane {
    pub(super) fn new(changeset: &Changeset) -> Self {
        Self {
            state: DiffPaneState::new(changeset),
            render: DiffRenderState::new(changeset.files.len()),
            viewport: DiffViewport::new(),
            scrollbar_drag: None,
        }
    }

    /// Invalidates pointer geometry before the application chooses which pane
    /// to render. A review render publishes fresh geometry later in the frame.
    pub(super) fn invalidate_frame(&mut self) {
        self.viewport.invalidate_frame();
    }

    pub(super) fn clear_render_cache(&mut self, file_count: usize) {
        self.render.clear(file_count);
        self.viewport.clear();
        self.scrollbar_drag = None;
    }

    pub(super) fn render(&mut self, request: SelectedReviewFrame<'_>) -> RenderedSelectedDiff {
        let SelectedReviewFrame {
            area,
            content_width,
            visible_height,
            theme,
            can_stage,
            file_index,
            changeset,
            source,
            no_diff_message,
            mut leading_rows,
        } = request;

        let provisional_height = visible_height.saturating_sub(
            leading_rows.len() + self.search_status_lines(content_width, theme).len(),
        );
        let mut diff_content_width = self.content_width(
            changeset,
            file_index,
            content_width,
            provisional_height,
            theme,
            can_stage,
        );

        let pending_search_scroll = self.prepare_search_scroll(
            changeset,
            source,
            file_index,
            area,
            diff_content_width,
            provisional_height,
            theme,
            can_stage,
        );

        leading_rows.extend(self.search_status_lines(content_width, theme));
        self.viewport.set_status_rows(leading_rows.len());
        let status_rows = self.viewport.status_rows();
        let visible_diff_height = visible_height.saturating_sub(status_rows);
        diff_content_width = self.content_width(
            changeset,
            file_index,
            content_width,
            visible_diff_height,
            theme,
            can_stage,
        );
        let total_rows =
            self.total_rows(changeset, file_index, diff_content_width, theme, can_stage);

        self.viewport.begin(area, visible_diff_height);
        self.update_scrollbar(
            changeset,
            file_index,
            area,
            content_width,
            visible_diff_height,
            total_rows,
            status_rows,
        );
        self.ensure_bounds(file_index, changeset.files.get(file_index));

        if pending_search_scroll {
            self.state.scroll_active_search_match(
                &self.render,
                self.viewport.view_height(),
                file_index,
                changeset.files.get(file_index),
            );
            self.ensure_bounds(file_index, changeset.files.get(file_index));
        }

        if visible_diff_height > 0 {
            self.ensure_cache(
                changeset,
                source,
                file_index,
                diff_content_width,
                visible_diff_height,
                theme,
                can_stage,
            );
            leading_rows.extend(self.visible_lines(
                changeset,
                file_index,
                diff_content_width,
                visible_diff_height,
                theme,
                can_stage,
                no_diff_message,
            ));
        }

        leading_rows.truncate(visible_height);
        self.update_scrollbar(
            changeset,
            file_index,
            area,
            content_width,
            visible_diff_height,
            total_rows,
            status_rows,
        );

        RenderedSelectedDiff {
            lines: leading_rows,
            scrollbar: self.viewport.scrollbar().cloned(),
        }
    }

    pub(super) fn selected_hunk<'a>(&self, file: Option<&'a DiffFile>) -> Option<&'a DiffHunk> {
        self.state.selected_hunk(file)
    }

    pub(super) fn selected_hunk_index(&self) -> Option<usize> {
        self.state.selected_hunk_index()
    }

    #[cfg(test)]
    pub(super) fn scroll(&self) -> usize {
        self.state.scroll()
    }

    pub(super) fn restore_position(&mut self, hunk_index: Option<usize>, scroll: usize) {
        self.state.set_selected_hunk_index(hunk_index);
        self.state.set_scroll(scroll);
    }

    pub(super) fn capture_reload_position(&self, file: Option<&DiffFile>) -> DiffReloadPosition {
        DiffReloadPosition {
            hunk_identity: self.selected_hunk(file).map(HunkIdentity::from),
            hunk_index: self.state.selected_hunk_index(),
            scroll: self.state.scroll(),
        }
    }

    pub(super) fn review_reloaded(
        &mut self,
        file: Option<&DiffFile>,
        kept_file_selection: bool,
        preserve_scroll: bool,
        previous: DiffReloadPosition,
    ) {
        let hunk_index = reloaded_hunk_index(file, kept_file_selection, previous);
        let scroll = if preserve_scroll && kept_file_selection {
            previous.scroll
        } else {
            0
        };
        self.restore_position(hunk_index, scroll);
        self.state.invalidate_search_matches();
        self.viewport.clear();
        self.scrollbar_drag = None;
    }

    pub(super) fn select_file(&mut self, file: Option<&DiffFile>) {
        self.state.select_file(file);
        self.viewport.clear();
        self.scrollbar_drag = None;
    }

    pub(super) fn ensure_bounds(&mut self, file_index: usize, file: Option<&DiffFile>) {
        let file_id = file.map(|file| file.id.as_str());
        let render_max = self.render.max_scroll(
            file_index,
            file_id,
            file.map_or(0, DiffFile::line_count),
            self.viewport.view_height(),
        );
        let scrollbar_max = file_id
            .map(|file_id| self.viewport.scrollbar_max_scroll(file_index, file_id))
            .unwrap_or(0);
        self.state
            .ensure_bounds(file, render_max.max(scrollbar_max));
    }

    pub(super) fn scroll_page(
        &mut self,
        direction: VerticalDirection,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.state.scroll_page(
            direction,
            self.viewport.view_height(),
            &self.render,
            file_index,
            file,
        );
    }

    pub(super) fn scroll_by(
        &mut self,
        direction: VerticalDirection,
        amount: usize,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.state
            .scroll_by(direction, amount, &self.render, file_index, file);
    }

    pub(super) fn scroll_to(&mut self, scroll: usize, file_index: usize, file: Option<&DiffFile>) {
        self.state.scroll_to(scroll, &self.render, file_index, file);
    }

    pub(super) fn scroll_to_top(&mut self, file_index: usize, file: Option<&DiffFile>) {
        self.state.scroll_to_top(&self.render, file_index, file);
    }

    pub(super) fn scroll_to_bottom(&mut self, file: Option<&DiffFile>) {
        self.state.scroll_to_bottom(file);
    }

    pub(super) fn jump_by(
        &mut self,
        direction: VerticalDirection,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.state.jump_by(
            direction,
            self.viewport.view_height(),
            &self.render,
            file_index,
            file,
        );
    }

    pub(super) fn select_hunk(&mut self, index: usize, file_index: usize, file: Option<&DiffFile>) {
        self.state.set_selected_hunk_index(Some(index));
        self.state.center_selected_hunk(
            &self.render,
            self.viewport.view_height(),
            file_index,
            file,
        );
    }

    pub(super) fn editor_line(&self, file_index: usize, file: &DiffFile) -> Option<u32> {
        self.state
            .editor_line(self.viewport.view_height(), &self.render, file_index, file)
    }

    pub(super) fn contains(&self, column: u16, row: u16) -> bool {
        self.viewport.contains(column, row)
    }

    pub(super) fn hunk_index_at(
        &self,
        column: u16,
        row: u16,
        file_index: usize,
        file: Option<&DiffFile>,
    ) -> Option<usize> {
        let visible_row = self.viewport.visible_row_at(column, row)?;
        let diff_row = visible_row.checked_sub(self.viewport.status_rows())?;
        self.state.hunk_index_at_rendered_row(
            &self.render,
            file_index,
            file,
            self.state.scroll().saturating_add(diff_row),
        )
    }

    pub(super) fn start_scrollbar_drag(&mut self, column: u16, row: u16) -> Option<usize> {
        self.scrollbar_drag = None;
        let scrollbar = self.viewport.scrollbar()?;
        let drag = scrollbar.drag_at(column, row)?;
        let scroll = scrollbar.scroll_for_drag(row, drag);
        self.scrollbar_drag = Some(drag);
        Some(scroll)
    }

    pub(super) fn drag_scrollbar(&mut self, row: u16) -> Option<usize> {
        let drag = self.scrollbar_drag?;
        let Some(scrollbar) = self.viewport.scrollbar() else {
            self.scrollbar_drag = None;
            return None;
        };
        Some(scrollbar.scroll_for_drag(row, drag))
    }

    pub(super) fn finish_scrollbar_drag(&mut self) -> bool {
        self.scrollbar_drag.take().is_some()
    }

    pub(super) fn cancel_scrollbar_drag(&mut self) {
        self.scrollbar_drag = None;
    }

    pub(super) fn search_prompt_open(&self) -> bool {
        self.state.search_prompt_open()
    }

    pub(super) fn open_search_prompt(&mut self) {
        self.state.open_search_prompt();
    }

    pub(super) fn handle_search_prompt_key(&mut self, key: KeyEvent) {
        self.state.handle_search_prompt_key(key);
    }

    pub(super) fn clear_search_query(&mut self) {
        self.state.clear_search_query();
    }

    fn search_status_lines(&self, content_width: usize, theme: Theme) -> Vec<Line<'static>> {
        rows::search_status_lines(self.state.search_status(), content_width, theme)
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_search_scroll(
        &mut self,
        changeset: &mut Changeset,
        source: &ReviewSource,
        file_index: usize,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
        can_stage: bool,
    ) -> bool {
        if !self.state.has_active_search() {
            return false;
        }
        self.viewport.begin(area, visible_height);
        self.ensure_bounds(file_index, changeset.files.get(file_index));
        self.ensure_cache(
            changeset,
            source,
            file_index,
            content_width,
            visible_height,
            theme,
            can_stage,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_cache(
        &mut self,
        changeset: &mut Changeset,
        source: &ReviewSource,
        file_index: usize,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
        can_stage: bool,
    ) -> bool {
        if file_index >= changeset.files.len() {
            self.state.clear_rendered_search_matches();
            return false;
        }

        let requested_rows = self.state.render_target_rows(visible_height);
        self.render.ensure_selected_file(
            changeset,
            source,
            SelectedDiffRenderRequest {
                file_index,
                content_width,
                theme,
                can_stage,
                requested_rows,
            },
        );
        let pending = self.state.refresh_search_matches(
            &self.render,
            file_index,
            changeset.files.get(file_index),
        );
        self.ensure_bounds(file_index, changeset.files.get(file_index));
        pending
    }

    fn total_rows(
        &mut self,
        changeset: &Changeset,
        file_index: usize,
        content_width: usize,
        theme: Theme,
        can_stage: bool,
    ) -> usize {
        self.render.total_rows(
            file_index,
            changeset.files.get(file_index),
            content_width,
            theme,
            can_stage,
        )
    }

    fn content_width(
        &mut self,
        changeset: &Changeset,
        file_index: usize,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
        can_stage: bool,
    ) -> usize {
        if content_width > 1
            && visible_height > 0
            && self.total_rows(changeset, file_index, content_width, theme, can_stage)
                > visible_height
        {
            content_width - 1
        } else {
            content_width
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn visible_lines(
        &self,
        changeset: &Changeset,
        file_index: usize,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
        can_stage: bool,
        no_diff_message: &'static str,
    ) -> Vec<Line<'static>> {
        let Some(file) = changeset.files.get(file_index) else {
            return rows::no_diff_lines(no_diff_message, content_width, theme);
        };
        let lines = self.render.visible_selected_lines(
            file_index,
            file,
            self.state.scroll(),
            visible_height,
            content_width,
            theme,
            can_stage,
            self.state.selected_hunk_index(),
        );
        self.state.highlight_search_matches(lines, theme)
    }

    #[allow(clippy::too_many_arguments)]
    fn update_scrollbar(
        &mut self,
        changeset: &Changeset,
        file_index: usize,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        total_rows: usize,
        status_rows: usize,
    ) {
        let scrollbar = if content_width <= 1 || visible_height == 0 || total_rows <= visible_height
        {
            None
        } else {
            changeset.files.get(file_index).map(|file| {
                DiffScrollbar::new(
                    Rect {
                        x: area.x.saturating_add(area.width.saturating_sub(2)),
                        y: area
                            .y
                            .saturating_add(1)
                            .saturating_add(saturating_u16(status_rows)),
                        width: 1,
                        height: saturating_u16(visible_height),
                    },
                    file_index,
                    file.id.clone(),
                    total_rows,
                    visible_height,
                    self.state.scroll(),
                )
            })
        };
        self.viewport.set_scrollbar(scrollbar);
    }

    #[cfg(test)]
    pub(super) fn set_scroll(&mut self, scroll: usize) {
        self.state.set_scroll(scroll);
    }

    #[cfg(test)]
    pub(super) fn set_selected_hunk_index(&mut self, index: Option<usize>) {
        self.state.set_selected_hunk_index(index);
    }

    #[cfg(test)]
    pub(super) fn begin_test_frame(&mut self, area: Rect, height: usize) {
        self.viewport.begin(area, height);
    }

    #[cfg(test)]
    pub(super) fn cache_test_diff_lines(
        &mut self,
        file_index: usize,
        lines: crate::diff_render::RenderedDiffLines,
    ) {
        self.render.cache_test_diff_lines(file_index, lines);
    }

    #[cfg(test)]
    pub(super) fn hunk_offset(
        &self,
        file_index: usize,
        file_id: &str,
        hunk_index: usize,
    ) -> Option<usize> {
        self.render.hunk_offset(file_index, file_id, hunk_index)
    }

    #[cfg(test)]
    pub(super) fn view_height(&self) -> usize {
        self.viewport.view_height()
    }

    #[cfg(test)]
    pub(super) fn search_match_count(&self) -> usize {
        self.state.search_match_count()
    }

    #[cfg(test)]
    pub(super) fn active_search_index(&self) -> Option<usize> {
        self.state.active_search_index()
    }

    #[cfg(test)]
    pub(super) fn active_search_match_row(&self) -> Option<usize> {
        self.state.active_search_match_row()
    }

    #[cfg(test)]
    pub(super) fn active_search_query(&self) -> Option<&str> {
        self.state.active_search_query()
    }
}

impl From<&DiffHunk> for HunkIdentity {
    fn from(hunk: &DiffHunk) -> Self {
        Self {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
        }
    }
}

fn reloaded_hunk_index(
    file: Option<&DiffFile>,
    kept_file_selection: bool,
    previous: DiffReloadPosition,
) -> Option<usize> {
    let file = file?;
    if file.hunks.is_empty() {
        return None;
    }
    if !kept_file_selection {
        return Some(0);
    }
    if let Some(index) = previous.hunk_identity.and_then(|identity| {
        file.hunks
            .iter()
            .position(|hunk| HunkIdentity::from(hunk) == identity)
    }) {
        return Some(index);
    }
    previous
        .hunk_index
        .map(|index| index.min(file.hunks.len() - 1))
        .or(Some(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileStage, FileStatus, SourceSnapshot};

    #[test]
    fn reload_position_follows_hunk_coordinates_after_reordering() {
        let previous_hunk = hunk(20, 2, 30, 2);
        let previous = DiffReloadPosition {
            hunk_identity: Some(HunkIdentity::from(&previous_hunk)),
            hunk_index: Some(1),
            scroll: 12,
        };
        let file = file(vec![previous_hunk.clone(), hunk(4, 1, 8, 1)]);

        assert_eq!(reloaded_hunk_index(Some(&file), true, previous), Some(0));
    }

    #[test]
    fn reload_position_clamps_index_when_coordinates_disappear() {
        let previous = DiffReloadPosition {
            hunk_identity: Some(HunkIdentity::from(&hunk(20, 2, 30, 2))),
            hunk_index: Some(4),
            scroll: 12,
        };
        let file = file(vec![hunk(4, 1, 8, 1)]);

        assert_eq!(reloaded_hunk_index(Some(&file), true, previous), Some(0));
        assert_eq!(reloaded_hunk_index(Some(&file), false, previous), Some(0));
    }

    fn file(hunks: Vec<DiffHunk>) -> DiffFile {
        DiffFile {
            id: "sample.rs".to_string(),
            old_path: "sample.rs".to_string(),
            path: "sample.rs".to_string(),
            old_source: SourceSnapshot::default(),
            new_source: SourceSnapshot::default(),
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks,
            binary: false,
        }
    }

    fn hunk(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> DiffHunk {
        DiffHunk {
            header: String::new(),
            old_start,
            old_lines,
            new_start,
            new_lines,
            stage: FileStage::Unstaged,
            lines: Vec::new(),
        }
    }
}
