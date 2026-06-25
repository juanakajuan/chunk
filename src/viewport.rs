//! Rendered viewport geometry for terminal panes.
//!
//! This module owns terminal geometry, row hit-testing, scroll clamping, and
//! scrollbar geometry. `diff_render` owns wrapped diff rows and layout metrics.

use ratatui::layout::Rect;

use crate::rows::SidebarRowTarget;

#[derive(Debug)]
pub struct RenderedViewport {
    /// Current diff viewport height, updated by the renderer.
    diff_view_height: usize,
    /// Current sidebar viewport height, updated by the renderer.
    sidebar_view_height: usize,
    /// Last sidebar rectangle, used to map mouse events.
    sidebar_area: Option<Rect>,
    /// Last diff rectangle, used to map mouse events.
    diff_area: Option<Rect>,
    /// Last rendered diff scrollbar, used to draw and map mouse events.
    diff_scrollbar: Option<DiffScrollbar>,
    /// Live-status rows stacked above the diff rows, used to map mouse events
    /// back to diff rows. Produced by the diff frame, read by hit-testing.
    diff_status_rows: usize,
    /// Rendered sidebar row to file or folder target mapping for click handling.
    sidebar_row_targets: Vec<SidebarRowTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportScrollInput {
    pub diff_scroll: usize,
    pub sidebar_scroll: usize,
    pub file_count: usize,
    pub diff_max_scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportScrollState {
    pub diff_scroll: usize,
    pub sidebar_scroll: usize,
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
            diff_view_height: 1,
            sidebar_view_height: 1,
            sidebar_area: None,
            diff_area: None,
            diff_scrollbar: None,
            diff_status_rows: 0,
            sidebar_row_targets: Vec::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.sidebar_area = None;
        self.diff_area = None;
        self.diff_scrollbar = None;
        self.sidebar_row_targets.clear();
    }

    pub fn clear_diff_geometry(&mut self) {
        self.diff_scrollbar = None;
    }

    pub fn diff_view_height(&self) -> usize {
        self.diff_view_height
    }

    pub fn begin_sidebar(&mut self, area: Rect, height: usize) {
        self.sidebar_area = Some(area);
        self.sidebar_view_height = height.max(1);
        self.sidebar_row_targets.clear();
    }

    pub fn begin_diff(&mut self, area: Rect, height: usize) {
        self.diff_area = Some(area);
        self.diff_view_height = height.max(1);
    }

    pub fn set_diff_scrollbar(&mut self, scrollbar: Option<DiffScrollbar>) {
        self.diff_scrollbar = scrollbar;
    }

    pub fn diff_scrollbar(&self) -> Option<&DiffScrollbar> {
        self.diff_scrollbar.as_ref()
    }

    pub fn set_diff_status_rows(&mut self, rows: usize) {
        self.diff_status_rows = rows;
    }

    pub fn diff_status_rows(&self) -> usize {
        self.diff_status_rows
    }

    pub fn begin_sidebar_rows(&mut self) {
        self.sidebar_row_targets.clear();
    }

    pub fn record_sidebar_rows(&mut self, target: SidebarRowTarget, row_count: usize) {
        self.sidebar_row_targets
            .extend(std::iter::repeat_n(target, row_count));
    }

    pub fn sidebar_target_at(&self, column: u16, row: u16) -> Option<SidebarRowTarget> {
        let area = self.sidebar_area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }

        let row_offset = row.saturating_sub(area.y + 1) as usize;
        self.sidebar_row_targets.get(row_offset).cloned()
    }

    pub fn is_sidebar_at(&self, column: u16, row: u16) -> bool {
        self.sidebar_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub fn is_diff_at(&self, column: u16, row: u16) -> bool {
        self.diff_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    pub fn diff_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.diff_area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }

        Some(row.saturating_sub(area.y + 1) as usize)
    }

    pub fn clamped_scrolls(&self, input: ViewportScrollInput) -> ViewportScrollState {
        let diff_scroll = input.diff_scroll.min(input.diff_max_scroll);
        let sidebar_scroll = if input.file_count == 0 {
            0
        } else {
            input.sidebar_scroll
        };

        ViewportScrollState {
            diff_scroll,
            sidebar_scroll,
        }
    }

    pub fn diff_scrollbar_max_scroll(&self, file_index: usize, file_id: &str) -> usize {
        self.matching_diff_scrollbar(file_index, Some(file_id))
            .map_or(0, DiffScrollbar::max_scroll)
    }

    fn matching_diff_scrollbar(
        &self,
        file_index: usize,
        file_id: Option<&str>,
    ) -> Option<&DiffScrollbar> {
        let file_id = file_id?;
        self.diff_scrollbar
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
    use crate::rows::SidebarRowTarget;

    #[test]
    fn clamped_scrolls_leave_sidebar_scroll_for_sidebar_renderer() {
        let mut viewport = RenderedViewport::new();
        viewport.begin_sidebar(Rect::default(), 3);

        let scrolls = viewport.clamped_scrolls(ViewportScrollInput {
            diff_scroll: 0,
            sidebar_scroll: 0,
            file_count: 6,
            diff_max_scroll: 0,
        });
        assert_eq!(scrolls.sidebar_scroll, 0);

        let scrolls = viewport.clamped_scrolls(ViewportScrollInput {
            diff_scroll: 0,
            sidebar_scroll: 99,
            file_count: 6,
            diff_max_scroll: 0,
        });
        assert_eq!(scrolls.sidebar_scroll, 99);
    }

    #[test]
    fn sidebar_row_mapping_records_visible_rows() {
        let mut viewport = RenderedViewport::new();
        viewport.begin_sidebar(Rect::new(0, 0, 12, 5), 3);
        viewport.record_sidebar_rows(SidebarRowTarget::File(2), 3);

        assert_eq!(
            viewport.sidebar_target_at(1, 1),
            Some(SidebarRowTarget::File(2))
        );
        assert_eq!(
            viewport.sidebar_target_at(1, 3),
            Some(SidebarRowTarget::File(2))
        );
        assert_eq!(viewport.sidebar_target_at(1, 4), None);
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
