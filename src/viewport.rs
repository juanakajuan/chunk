//! Rendered viewport geometry for terminal panes.
//!
//! This module owns terminal geometry and row hit-testing. The application
//! viewport tracks the navigation panels, while `DiffViewport` is owned by the
//! selected-diff module together with its scrollbar interaction state.

use ratatui::layout::Rect;

#[derive(Debug)]
pub struct RenderedViewport {
    /// Last branches rectangle and its visible item range.
    branches_area: Option<Rect>,
    branches_row_indices: Vec<usize>,
    /// Last commits rectangle and its visible item range.
    commits_area: Option<Rect>,
    commits_row_indices: Vec<usize>,
}

#[derive(Debug)]
pub struct DiffViewport {
    view_height: usize,
    area: Option<Rect>,
    scrollbar: Option<DiffScrollbar>,
    /// Status rows stacked above diff content in the pane's inner area.
    status_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffScrollbar {
    area: Rect,
    file_index: usize,
    file_id: String,
    total_rows: usize,
    visible_rows: usize,
    scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffScrollbarThumb {
    pub start: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffScrollbarDrag {
    thumb_offset: usize,
}

impl RenderedViewport {
    pub fn new() -> Self {
        Self {
            branches_area: None,
            branches_row_indices: Vec::new(),
            commits_area: None,
            commits_row_indices: Vec::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.branches_area = None;
        self.branches_row_indices.clear();
        self.commits_area = None;
        self.commits_row_indices.clear();
    }

    pub fn begin_branches(&mut self, area: Rect, first_index: usize, row_count: usize) {
        self.branches_area = Some(area);
        self.branches_row_indices = (first_index..first_index.saturating_add(row_count)).collect();
    }

    pub fn begin_commits(&mut self, area: Rect, first_index: usize, row_count: usize) {
        self.commits_area = Some(area);
        self.commits_row_indices = (first_index..first_index.saturating_add(row_count)).collect();
    }

    pub fn branch_index_at(&self, column: u16, row: u16) -> Option<usize> {
        panel_index_at(self.branches_area?, &self.branches_row_indices, column, row)
    }

    pub fn commit_index_at(&self, column: u16, row: u16) -> Option<usize> {
        panel_index_at(self.commits_area?, &self.commits_row_indices, column, row)
    }

    pub fn is_branches_at(&self, column: u16, row: u16) -> bool {
        self.branches_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub fn is_commits_at(&self, column: u16, row: u16) -> bool {
        self.commits_area
            .is_some_and(|area| rect_contains(area, column, row))
    }
}

impl DiffViewport {
    pub fn new() -> Self {
        Self {
            view_height: 1,
            area: None,
            scrollbar: None,
            status_rows: 0,
        }
    }

    pub fn invalidate_frame(&mut self) {
        self.area = None;
        self.scrollbar = None;
    }

    pub fn clear(&mut self) {
        self.invalidate_frame();
        self.status_rows = 0;
    }

    pub fn view_height(&self) -> usize {
        self.view_height
    }

    pub fn begin(&mut self, area: Rect, height: usize) {
        self.area = Some(area);
        self.view_height = height.max(1);
    }

    pub fn set_scrollbar(&mut self, scrollbar: Option<DiffScrollbar>) {
        self.scrollbar = scrollbar;
    }

    pub fn scrollbar(&self) -> Option<&DiffScrollbar> {
        self.scrollbar.as_ref()
    }

    pub fn set_status_rows(&mut self, rows: usize) {
        self.status_rows = rows;
    }

    pub fn status_rows(&self) -> usize {
        self.status_rows
    }

    pub fn contains(&self, column: u16, row: u16) -> bool {
        self.area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub fn visible_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }

        Some(row.saturating_sub(area.y + 1) as usize)
    }

    pub fn scrollbar_max_scroll(&self, file_index: usize, file_id: &str) -> usize {
        self.matching_scrollbar(file_index, Some(file_id))
            .map_or(0, DiffScrollbar::max_scroll)
    }

    fn matching_scrollbar(
        &self,
        file_index: usize,
        file_id: Option<&str>,
    ) -> Option<&DiffScrollbar> {
        let file_id = file_id?;
        self.scrollbar
            .as_ref()
            .filter(|scrollbar| scrollbar.matches_file(file_index, file_id))
    }
}

impl DiffScrollbar {
    pub fn new(
        area: Rect,
        file_index: usize,
        file_id: String,
        total_rows: usize,
        visible_rows: usize,
        scroll: usize,
    ) -> Self {
        Self {
            area,
            file_index,
            file_id,
            total_rows,
            visible_rows,
            scroll,
        }
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn thumb(&self) -> DiffScrollbarThumb {
        let track_height = self.track_height();
        let thumb_len = ratio_ceil(self.visible_rows, track_height, self.total_rows)
            .max(1)
            .min(track_height);
        let track_range = track_height.saturating_sub(thumb_len);
        let scroll_range = self.max_scroll();
        let start = if track_range == 0 || scroll_range == 0 {
            0
        } else {
            ratio_round(self.scroll.min(scroll_range), track_range, scroll_range)
        };

        DiffScrollbarThumb {
            start,
            len: thumb_len,
        }
    }

    pub fn drag_at(&self, column: u16, row: u16) -> Option<DiffScrollbarDrag> {
        if !rect_contains(self.area, column, row) {
            return None;
        }

        let row_offset = self.row_offset(row);
        let thumb = self.thumb();
        let thumb_end = thumb.start.saturating_add(thumb.len);
        let thumb_offset = if row_offset >= thumb.start && row_offset < thumb_end {
            row_offset.saturating_sub(thumb.start)
        } else {
            thumb.len / 2
        };

        Some(DiffScrollbarDrag { thumb_offset })
    }

    pub fn scroll_for_drag(&self, row: u16, drag: DiffScrollbarDrag) -> usize {
        let thumb = self.thumb();
        let track_range = self.track_height().saturating_sub(thumb.len);
        let scroll_range = self.max_scroll();
        if track_range == 0 || scroll_range == 0 {
            return 0;
        }

        let thumb_start = self
            .row_offset(row)
            .saturating_sub(drag.thumb_offset)
            .min(track_range);
        ratio_round(thumb_start, scroll_range, track_range)
    }

    fn matches_file(&self, file_index: usize, file_id: &str) -> bool {
        self.file_index == file_index && self.file_id == file_id
    }

    fn max_scroll(&self) -> usize {
        self.total_rows.saturating_sub(self.visible_rows)
    }

    fn row_offset(&self, row: u16) -> usize {
        row.saturating_sub(self.area.y)
            .min(self.area.height.saturating_sub(1)) as usize
    }

    fn track_height(&self) -> usize {
        self.area.height.max(1) as usize
    }
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn rect_inner_contains(area: Rect, column: u16, row: u16) -> bool {
    column > area.x
        && column < area.x.saturating_add(area.width).saturating_sub(1)
        && row > area.y
        && row < area.y.saturating_add(area.height).saturating_sub(1)
}

fn panel_index_at(area: Rect, row_indices: &[usize], column: u16, row: u16) -> Option<usize> {
    if !rect_inner_contains(area, column, row) {
        return None;
    }

    let row_offset = row.saturating_sub(area.y + 1) as usize;
    row_indices.get(row_offset).copied()
}

fn ratio_ceil(value: usize, numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return 0;
    }

    let denominator = denominator as u128;
    let scaled = (value as u128).saturating_mul(numerator as u128);
    scaled.div_ceil(denominator).min(usize::MAX as u128) as usize
}

fn ratio_round(value: usize, numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return 0;
    }

    let denominator = denominator as u128;
    let scaled = (value as u128).saturating_mul(numerator as u128);
    let rounded = scaled.saturating_add(denominator / 2) / denominator;
    rounded.min(usize::MAX as u128) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_panel_mapping_includes_scroll_offset() {
        let mut viewport = RenderedViewport::new();
        viewport.begin_branches(Rect::new(0, 4, 20, 5), 3, 3);
        viewport.begin_commits(Rect::new(0, 9, 20, 4), 8, 2);

        assert_eq!(viewport.branch_index_at(1, 5), Some(3));
        assert_eq!(viewport.branch_index_at(1, 7), Some(5));
        assert_eq!(viewport.commit_index_at(1, 10), Some(8));
        assert_eq!(viewport.commit_index_at(1, 12), None);
    }

    #[test]
    fn diff_scrollbar_thumb_reflects_scroll_and_visible_rows() {
        let scrollbar =
            DiffScrollbar::new(Rect::new(0, 0, 1, 10), 0, "file".to_string(), 100, 20, 40);

        let thumb = scrollbar.thumb();
        assert_eq!(thumb.len, 2);
        assert_eq!(thumb.start, 4);

        let drag = scrollbar.drag_at(0, 9).unwrap();
        assert_eq!(scrollbar.scroll_for_drag(9, drag), 80);
    }
}
