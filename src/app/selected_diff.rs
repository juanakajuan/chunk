//! The selected review diff as one stateful module.
//!
//! This is the ownership seam for diff-local state: scroll and hunk
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
use crate::search::Search;
use crate::theme::Theme;
use crate::viewport::{DiffScrollbar, DiffScrollbarDrag, DiffViewport};

use super::saturating_u16;

#[derive(Debug)]
pub(super) struct SelectedDiffPane {
    scroll: usize,
    selected_hunk_index: Option<usize>,
    search: Search,
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

#[derive(Clone, Copy)]
struct DiffRenderContext<'a> {
    render: &'a DiffRenderState,
    file_index: usize,
    file: Option<&'a DiffFile>,
}

impl<'a> DiffRenderContext<'a> {
    fn new(render: &'a DiffRenderState, file_index: usize, file: Option<&'a DiffFile>) -> Self {
        Self {
            render,
            file_index,
            file,
        }
    }

    fn hunk_index_at(self, rendered_row: usize) -> Option<usize> {
        let file = self.file?;
        self.render.hunk_index_at(
            self.file_index,
            file.id.as_str(),
            rendered_row,
            file.hunks.len(),
        )
    }

    fn hunk_offset(self, hunk_index: usize) -> Option<usize> {
        let file = self.file?;
        self.render
            .hunk_offset(self.file_index, file.id.as_str(), hunk_index)
    }

    fn new_line_at(self, rendered_row: usize) -> Option<u32> {
        let file = self.file?;
        self.render
            .new_line_at(self.file_index, file.id.as_str(), rendered_row)
    }
}

impl SelectedDiffPane {
    pub(super) fn new(changeset: &Changeset) -> Self {
        Self {
            scroll: 0,
            selected_hunk_index: initial_selected_hunk_index(changeset),
            search: Search::default(),
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
        self.clear_view_state();
    }

    fn clear_view_state(&mut self) {
        self.viewport.clear();
        self.scrollbar_drag = None;
    }

    pub(super) fn render(&mut self, mut frame: SelectedReviewFrame<'_>) -> RenderedSelectedDiff {
        let provisional_height = frame.visible_height.saturating_sub(
            frame.leading_rows.len()
                + self
                    .search_status_lines(frame.content_width, frame.theme)
                    .len(),
        );
        let mut diff_content_width = self.content_width(&frame, provisional_height);

        let pending_search_scroll =
            self.prepare_search_scroll(&mut frame, diff_content_width, provisional_height);

        frame
            .leading_rows
            .extend(self.search_status_lines(frame.content_width, frame.theme));
        let status_rows = frame.leading_rows.len();
        self.viewport.set_status_rows(status_rows);
        let visible_diff_height = frame.visible_height.saturating_sub(status_rows);
        diff_content_width = self.content_width(&frame, visible_diff_height);
        let total_rows = self.total_rows(&frame, diff_content_width);

        self.viewport.begin(frame.area, visible_diff_height);
        self.update_scrollbar(&frame, visible_diff_height, total_rows, status_rows);
        self.ensure_bounds(
            frame.file_index,
            frame.changeset.files.get(frame.file_index),
        );

        if pending_search_scroll {
            self.scroll_active_search_match(
                frame.file_index,
                frame.changeset.files.get(frame.file_index),
            );
            self.ensure_bounds(
                frame.file_index,
                frame.changeset.files.get(frame.file_index),
            );
        }

        if visible_diff_height > 0 {
            self.ensure_cache(&mut frame, diff_content_width, visible_diff_height);
            let visible_lines = self.visible_lines(&frame, diff_content_width, visible_diff_height);
            frame.leading_rows.extend(visible_lines);
        }

        frame.leading_rows.truncate(frame.visible_height);
        self.update_scrollbar(&frame, visible_diff_height, total_rows, status_rows);

        RenderedSelectedDiff {
            lines: frame.leading_rows,
            scrollbar: self.viewport.scrollbar().cloned(),
        }
    }

    pub(super) fn selected_hunk<'a>(&self, file: Option<&'a DiffFile>) -> Option<&'a DiffHunk> {
        file?.hunks.get(self.selected_hunk_index?)
    }

    pub(super) fn selected_hunk_index(&self) -> Option<usize> {
        self.selected_hunk_index
    }

    #[cfg(test)]
    pub(super) fn scroll(&self) -> usize {
        self.scroll
    }

    pub(super) fn restore_position(&mut self, hunk_index: Option<usize>, scroll: usize) {
        self.selected_hunk_index = hunk_index;
        self.scroll = scroll;
    }

    pub(super) fn capture_reload_position(&self, file: Option<&DiffFile>) -> DiffReloadPosition {
        DiffReloadPosition {
            hunk_identity: self.selected_hunk(file).map(HunkIdentity::from),
            hunk_index: self.selected_hunk_index,
            scroll: self.scroll,
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
        self.search.invalidate_matches();
        self.clear_view_state();
    }

    pub(super) fn select_file(&mut self, file: Option<&DiffFile>) {
        self.selected_hunk_index = file.and_then(|file| bounded_hunk_index(file, None));
        self.scroll = 0;
        self.search.invalidate_matches();
        self.clear_view_state();
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
        self.ensure_selected_hunk_bounds(file);
        self.scroll = self.scroll.min(render_max.max(scrollbar_max));
    }

    pub(super) fn scroll_page(
        &mut self,
        direction: VerticalDirection,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        self.scroll_by(direction, self.viewport.view_height(), file_index, file);
    }

    pub(super) fn scroll_by(
        &mut self,
        direction: VerticalDirection,
        amount: usize,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        let scroll = direction.shift(self.scroll, amount);
        let selected_hunk_index =
            DiffRenderContext::new(&self.render, file_index, file).hunk_index_at(scroll);
        self.scroll = scroll;
        self.selected_hunk_index = selected_hunk_index;
    }

    pub(super) fn scroll_to(&mut self, scroll: usize, file_index: usize, file: Option<&DiffFile>) {
        let selected_hunk_index =
            DiffRenderContext::new(&self.render, file_index, file).hunk_index_at(scroll);
        self.scroll = scroll;
        self.selected_hunk_index = selected_hunk_index;
    }

    pub(super) fn scroll_to_top(&mut self, file_index: usize, file: Option<&DiffFile>) {
        self.scroll_to(0, file_index, file);
    }

    pub(super) fn scroll_to_bottom(&mut self, file: Option<&DiffFile>) {
        self.scroll = usize::MAX;
        self.selected_hunk_index = file.and_then(|file| file.hunks.len().checked_sub(1));
    }

    pub(super) fn jump_by(
        &mut self,
        direction: VerticalDirection,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        if self.search.active_query().is_some() {
            self.jump_search_match(direction, file_index, file);
        } else {
            self.jump_hunk(direction, file_index, file);
        }
    }

    pub(super) fn select_hunk(&mut self, index: usize, file_index: usize, file: Option<&DiffFile>) {
        let offset = DiffRenderContext::new(&self.render, file_index, file).hunk_offset(index);
        self.selected_hunk_index = Some(index);
        if let Some(offset) = offset {
            self.scroll = offset.saturating_sub(self.viewport.view_height() / 2);
        }
    }

    pub(super) fn editor_line(&self, file_index: usize, file: &DiffFile) -> Option<u32> {
        let context = DiffRenderContext::new(&self.render, file_index, Some(file));
        let rendered_row = self.editor_target_row(context);
        context.new_line_at(rendered_row).or_else(|| {
            self.selected_hunk(Some(file))
                .and_then(DiffHunk::first_changed_line)
        })
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
        DiffRenderContext::new(&self.render, file_index, file)
            .hunk_index_at(self.scroll.saturating_add(diff_row))
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
        self.search.is_prompt_open()
    }

    pub(super) fn open_search_prompt(&mut self) {
        self.search.open_prompt();
    }

    pub(super) fn handle_search_prompt_key(&mut self, key: KeyEvent) {
        self.search.handle_prompt_key(key);
    }

    pub(super) fn clear_search_query(&mut self) {
        self.search.clear_query();
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
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        let Some(file) = file else {
            return;
        };
        if file.hunks.is_empty() {
            self.selected_hunk_index = None;
            return;
        }

        let context = DiffRenderContext::new(&self.render, file_index, Some(file));
        let current = self
            .selected_hunk_index
            .or_else(|| context.hunk_index_at(self.scroll))
            .unwrap_or(0)
            .min(file.hunks.len() - 1);
        let target = direction.shift_clamped(current, 1, file.hunks.len() - 1);
        let offset = context.hunk_offset(target);

        self.selected_hunk_index = Some(target);
        if let Some(offset) = offset {
            self.scroll = offset.saturating_sub(self.viewport.view_height() / 2);
        }
    }

    fn jump_search_match(
        &mut self,
        direction: VerticalDirection,
        file_index: usize,
        file: Option<&DiffFile>,
    ) {
        if self
            .search
            .advance_match(matches!(direction, VerticalDirection::Down))
        {
            self.scroll_active_search_match(file_index, file);
        }
    }

    fn scroll_active_search_match(&mut self, file_index: usize, file: Option<&DiffFile>) {
        let Some(active_row) = self.search.active_match_row() else {
            return;
        };
        let scroll = active_row.saturating_sub(self.viewport.view_height() / 2);
        let selected_hunk_index =
            DiffRenderContext::new(&self.render, file_index, file).hunk_index_at(scroll);
        self.scroll = scroll;
        self.selected_hunk_index = selected_hunk_index;
    }

    fn editor_target_row(&self, context: DiffRenderContext<'_>) -> usize {
        if let Some(active_row) = self.search.active_match_row() {
            return active_row;
        }

        self.selected_hunk_index
            .and_then(|hunk_index| context.hunk_offset(hunk_index))
            .filter(|hunk_offset| {
                *hunk_offset > self.scroll
                    && *hunk_offset < self.scroll.saturating_add(self.viewport.view_height())
            })
            .unwrap_or(self.scroll)
    }

    fn search_status_lines(&self, content_width: usize, theme: Theme) -> Vec<Line<'static>> {
        rows::search_status_lines(self.search.status(), content_width, theme)
    }

    fn prepare_search_scroll(
        &mut self,
        frame: &mut SelectedReviewFrame<'_>,
        content_width: usize,
        visible_height: usize,
    ) -> bool {
        if self.search.active_query().is_none() {
            return false;
        }
        self.viewport.begin(frame.area, visible_height);
        self.ensure_bounds(
            frame.file_index,
            frame.changeset.files.get(frame.file_index),
        );
        self.ensure_cache(frame, content_width, visible_height)
    }

    fn ensure_cache(
        &mut self,
        frame: &mut SelectedReviewFrame<'_>,
        content_width: usize,
        visible_height: usize,
    ) -> bool {
        if frame.file_index >= frame.changeset.files.len() {
            self.search.clear_rendered_matches();
            return false;
        }

        let requested_rows = if self.search.active_query().is_some() {
            usize::MAX
        } else {
            self.scroll
                .saturating_add(visible_height)
                .saturating_add(rows::DIFF_PREFETCH_ROWS)
        };
        self.render.ensure_selected_file(
            &mut *frame.changeset,
            frame.source,
            SelectedDiffRenderRequest {
                file_index: frame.file_index,
                content_width,
                theme: frame.theme,
                can_stage: frame.can_stage,
                requested_rows,
            },
        );
        let pending = self.render.refresh_search_matches(
            &mut self.search,
            frame.file_index,
            frame.changeset.files.get(frame.file_index),
        );
        self.ensure_bounds(
            frame.file_index,
            frame.changeset.files.get(frame.file_index),
        );
        pending
    }

    fn total_rows(&mut self, frame: &SelectedReviewFrame<'_>, content_width: usize) -> usize {
        self.render.total_rows(
            frame.file_index,
            frame.changeset.files.get(frame.file_index),
            content_width,
            frame.theme,
            frame.can_stage,
        )
    }

    fn content_width(&mut self, frame: &SelectedReviewFrame<'_>, visible_height: usize) -> usize {
        if frame.content_width > 1
            && visible_height > 0
            && self.total_rows(frame, frame.content_width) > visible_height
        {
            frame.content_width - 1
        } else {
            frame.content_width
        }
    }

    fn visible_lines(
        &self,
        frame: &SelectedReviewFrame<'_>,
        content_width: usize,
        visible_height: usize,
    ) -> Vec<Line<'static>> {
        let Some(file) = frame.changeset.files.get(frame.file_index) else {
            return rows::no_diff_lines(frame.no_diff_message, content_width, frame.theme);
        };
        let lines = self.render.visible_selected_lines(
            frame.file_index,
            file,
            self.scroll,
            visible_height,
            content_width,
            frame.theme,
            frame.can_stage,
            self.selected_hunk_index,
        );
        self.search.highlight(lines, self.scroll, frame.theme)
    }

    fn update_scrollbar(
        &mut self,
        frame: &SelectedReviewFrame<'_>,
        visible_height: usize,
        total_rows: usize,
        status_rows: usize,
    ) {
        let scrollbar =
            if frame.content_width <= 1 || visible_height == 0 || total_rows <= visible_height {
                None
            } else {
                frame.changeset.files.get(frame.file_index).map(|file| {
                    DiffScrollbar::new(
                        Rect {
                            x: frame
                                .area
                                .x
                                .saturating_add(frame.area.width.saturating_sub(2)),
                            y: frame
                                .area
                                .y
                                .saturating_add(1)
                                .saturating_add(saturating_u16(status_rows)),
                            width: 1,
                            height: saturating_u16(visible_height),
                        },
                        frame.file_index,
                        file.id.clone(),
                        total_rows,
                        visible_height,
                        self.scroll,
                    )
                })
            };
        self.viewport.set_scrollbar(scrollbar);
    }

    #[cfg(test)]
    pub(super) fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    #[cfg(test)]
    pub(super) fn set_selected_hunk_index(&mut self, index: Option<usize>) {
        self.selected_hunk_index = index;
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
    Some(
        previous
            .hunk_index
            .map_or(0, |index| index.min(file.hunks.len() - 1)),
    )
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
