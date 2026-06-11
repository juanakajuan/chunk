//! Rendered viewport state for terminal panes.
//!
//! This module owns geometry, row mapping, render caches, and scroll limits.
//! `App` owns review state; `ui` owns drawing.

use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::theme::SyntaxPalette;

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
    /// Rendered sidebar row to file index mapping for click handling.
    sidebar_row_indices: Vec<usize>,
    /// Cached sidebar row counts for the current sidebar layout.
    sidebar_row_counts_cache: Option<SidebarRowCountsCache>,
    /// Cached wrapped and highlighted diff lines by file index.
    diff_lines_cache: Vec<Option<RenderedDiffLines>>,
    /// Cached full diff row metrics by file index.
    diff_layout_cache: Vec<Option<CachedDiffLayoutMetrics>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportScrollInput<'a> {
    pub diff_scroll: usize,
    pub sidebar_scroll: usize,
    pub selected_file_index: usize,
    pub file_count: usize,
    pub selected_file_id: Option<&'a str>,
    pub selected_file_line_count: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffRenderRequest<'a> {
    pub file_index: usize,
    pub file_id: &'a str,
    pub content_width: usize,
    pub syntax_palette: SyntaxPalette,
    pub can_stage: bool,
    pub requested_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffLayoutRequest<'a> {
    pub file_index: usize,
    pub file_id: &'a str,
    pub content_width: usize,
    pub can_stage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLayoutMetrics {
    pub total_rows: usize,
    pub hunk_offsets: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct RenderedDiffLines {
    /// `DiffFile::id` for the cached file.
    file_id: String,
    /// Width used to wrap cached lines.
    content_width: usize,
    /// Syntax palette used while highlighting cached lines.
    syntax_palette: SyntaxPalette,
    /// Whether staging controls were rendered in the cached header.
    can_stage: bool,
    /// Rendered, wrapped lines for the selected file.
    lines: Vec<Line<'static>>,
    /// Rendered row offsets for each hunk header under this cache layout.
    hunk_offsets: Vec<usize>,
    /// Whether `lines` contains every rendered row for the file.
    complete: bool,
}

#[derive(Debug, Clone)]
struct CachedDiffLayoutMetrics {
    file_id: String,
    content_width: usize,
    can_stage: bool,
    metrics: DiffLayoutMetrics,
}

#[derive(Debug, Clone)]
struct SidebarRowCountsCache {
    /// Width used to wrap cached sidebar entries.
    content_width: usize,
    /// Whether row counts include staging controls.
    can_stage: bool,
    /// Wrapped row count for each file index.
    row_counts: Vec<usize>,
}

impl RenderedViewport {
    pub fn new(file_count: usize) -> Self {
        Self {
            diff_view_height: 1,
            sidebar_view_height: 1,
            sidebar_area: None,
            diff_area: None,
            diff_scrollbar: None,
            sidebar_row_indices: Vec::new(),
            sidebar_row_counts_cache: None,
            diff_lines_cache: vec![None; file_count],
            diff_layout_cache: vec![None; file_count],
        }
    }

    pub fn begin_frame(&mut self) {
        self.sidebar_area = None;
        self.diff_area = None;
        self.diff_scrollbar = None;
        self.sidebar_row_indices.clear();
    }

    pub fn clear_render_caches(&mut self, file_count: usize) {
        self.sidebar_row_counts_cache = None;
        self.diff_lines_cache = vec![None; file_count];
        self.diff_layout_cache = vec![None; file_count];
        self.diff_scrollbar = None;
    }

    pub fn diff_view_height(&self) -> usize {
        self.diff_view_height
    }

    pub fn begin_sidebar(&mut self, area: Rect, height: usize) {
        self.sidebar_area = Some(area);
        self.sidebar_view_height = height.max(1);
        self.sidebar_row_indices.clear();
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

    pub fn begin_sidebar_rows(&mut self) {
        self.sidebar_row_indices.clear();
    }

    pub fn record_sidebar_rows(&mut self, index: usize, row_count: usize) {
        self.sidebar_row_indices
            .extend(std::iter::repeat_n(index, row_count));
    }

    pub fn sidebar_index_at(&self, column: u16, row: u16, file_count: usize) -> Option<usize> {
        let area = self.sidebar_area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }

        let row_offset = row.saturating_sub(area.y + 1) as usize;
        self.sidebar_row_indices
            .get(row_offset)
            .copied()
            .filter(|index| *index < file_count)
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

    pub fn clamped_scrolls(&self, input: ViewportScrollInput<'_>) -> ViewportScrollState {
        let selected_file_index = input
            .selected_file_index
            .min(input.file_count.saturating_sub(1));
        let diff_scroll = input.diff_scroll.min(self.max_diff_scroll(input));
        let sidebar_scroll = input.sidebar_scroll.min(input.file_count.saturating_sub(1));
        let sidebar_scroll =
            self.sidebar_scroll_with_selected_visible(sidebar_scroll, selected_file_index);

        ViewportScrollState {
            diff_scroll,
            sidebar_scroll,
        }
    }

    fn max_diff_scroll(&self, input: ViewportScrollInput<'_>) -> usize {
        let rendered_scroll = self
            .rendered_diff_line_count(
                input.selected_file_index,
                input.selected_file_id,
                input.selected_file_line_count,
            )
            .saturating_sub(self.diff_view_height);
        let layout_scroll = self
            .matching_diff_layout_cache(input.selected_file_index, input.selected_file_id)
            .map(|cache| {
                cache
                    .metrics
                    .total_rows
                    .saturating_sub(self.diff_view_height)
            })
            .unwrap_or(0);
        let hunk_scroll = self
            .partial_cache_hunk_scroll_target(input.selected_file_index, input.selected_file_id);
        let scrollbar_scroll = self
            .matching_diff_scrollbar(input.selected_file_index, input.selected_file_id)
            .map_or(0, DiffScrollbar::max_scroll);

        rendered_scroll
            .max(layout_scroll)
            .max(hunk_scroll)
            .max(scrollbar_scroll)
    }

    fn sidebar_scroll_with_selected_visible(
        &self,
        sidebar_scroll: usize,
        selected_file_index: usize,
    ) -> usize {
        if selected_file_index < sidebar_scroll {
            return selected_file_index;
        }

        let visible_offset = self.sidebar_view_height.saturating_sub(1);
        if selected_file_index > sidebar_scroll + visible_offset {
            return selected_file_index.saturating_sub(visible_offset);
        }

        sidebar_scroll
    }

    fn rendered_diff_line_count(
        &self,
        file_index: usize,
        file_id: Option<&str>,
        fallback: usize,
    ) -> usize {
        let Some(cache) = self.matching_diff_lines_cache(file_index, file_id) else {
            return fallback;
        };

        if cache.complete {
            cache.len()
        } else {
            cache.len().max(fallback)
        }
    }

    pub fn ensure_diff_cache_len(&mut self, file_count: usize) {
        if self.diff_lines_cache.len() != file_count {
            self.diff_lines_cache = vec![None; file_count];
        }
        if self.diff_layout_cache.len() != file_count {
            self.diff_layout_cache = vec![None; file_count];
        }
    }

    pub fn diff_layout_metrics(
        &self,
        request: DiffLayoutRequest<'_>,
    ) -> Option<&DiffLayoutMetrics> {
        self.diff_layout_cache(request.file_index)
            .filter(|cache| cache.matches(request))
            .map(|cache| &cache.metrics)
    }

    pub fn cache_diff_layout_metrics(
        &mut self,
        request: DiffLayoutRequest<'_>,
        metrics: DiffLayoutMetrics,
    ) {
        if let Some(cache_slot) = self.diff_layout_cache.get_mut(request.file_index) {
            *cache_slot = Some(CachedDiffLayoutMetrics::new(request, metrics));
        }
    }

    pub fn diff_lines_render_target(&self, request: DiffRenderRequest<'_>) -> Option<usize> {
        let Some(cache) = self.diff_lines_cache(request.file_index) else {
            return Some(request.requested_rows);
        };

        cache.render_target_if_needed(request)
    }

    pub fn cache_diff_lines(&mut self, file_index: usize, lines: RenderedDiffLines) {
        if let Some(cache_slot) = self.diff_lines_cache.get_mut(file_index) {
            *cache_slot = Some(lines);
        }
    }

    pub fn visible_diff_lines(
        &self,
        file_index: usize,
        scroll: usize,
        visible_height: usize,
    ) -> Vec<Line<'static>> {
        self.diff_lines_cache(file_index)
            .map(|cache| cache.visible_lines(scroll, visible_height))
            .unwrap_or_default()
    }

    pub fn diff_lines(&self, file_index: usize, file_id: &str) -> Option<&[Line<'static>]> {
        self.matching_diff_lines_cache(file_index, Some(file_id))
            .map(|cache| cache.lines.as_slice())
    }

    pub fn diff_hunk_offsets(&self, file_index: usize, file_id: &str) -> Option<&[usize]> {
        self.matching_diff_lines_cache(file_index, Some(file_id))
            .map(|cache| cache.hunk_offsets.as_slice())
    }

    pub fn cached_sidebar_row_counts(
        &mut self,
        content_width: usize,
        can_stage: bool,
        file_count: usize,
        compute: impl FnOnce() -> Vec<usize>,
    ) -> &[usize] {
        let cache_matches = self
            .sidebar_row_counts_cache
            .as_ref()
            .is_some_and(|cache| cache.matches(content_width, can_stage, file_count));

        if !cache_matches {
            self.sidebar_row_counts_cache = Some(SidebarRowCountsCache {
                content_width,
                can_stage,
                row_counts: compute(),
            });
        }

        self.sidebar_row_counts_cache
            .as_ref()
            .map_or(&[], |cache| cache.row_counts.as_slice())
    }

    fn diff_lines_cache(&self, file_index: usize) -> Option<&RenderedDiffLines> {
        self.diff_lines_cache
            .get(file_index)
            .and_then(Option::as_ref)
    }

    fn diff_layout_cache(&self, file_index: usize) -> Option<&CachedDiffLayoutMetrics> {
        self.diff_layout_cache
            .get(file_index)
            .and_then(Option::as_ref)
    }

    fn matching_diff_lines_cache(
        &self,
        file_index: usize,
        file_id: Option<&str>,
    ) -> Option<&RenderedDiffLines> {
        let file_id = file_id?;
        self.diff_lines_cache(file_index)
            .filter(|cache| cache.matches_file(file_id))
    }

    fn matching_diff_layout_cache(
        &self,
        file_index: usize,
        file_id: Option<&str>,
    ) -> Option<&CachedDiffLayoutMetrics> {
        let file_id = file_id?;
        self.diff_layout_cache(file_index)
            .filter(|cache| cache.matches_file(file_id))
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

    fn partial_cache_hunk_scroll_target(&self, file_index: usize, file_id: Option<&str>) -> usize {
        self.matching_diff_lines_cache(file_index, file_id)
            .filter(|cache| !cache.complete)
            .and_then(|cache| cache.hunk_offsets.last().copied())
            .unwrap_or(0)
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

impl RenderedDiffLines {
    pub fn new(
        file_id: String,
        content_width: usize,
        syntax_palette: SyntaxPalette,
        can_stage: bool,
        lines: Vec<Line<'static>>,
        complete: bool,
    ) -> Self {
        Self {
            file_id,
            content_width,
            syntax_palette,
            can_stage,
            lines,
            hunk_offsets: Vec::new(),
            complete,
        }
    }

    pub fn with_hunk_offsets(mut self, hunk_offsets: Vec<usize>) -> Self {
        self.hunk_offsets = hunk_offsets;
        self
    }

    fn matches_file(&self, file_id: &str) -> bool {
        self.file_id == file_id
    }

    fn render_target_if_needed(&self, request: DiffRenderRequest<'_>) -> Option<usize> {
        if !self.matches_render_request(request) {
            return Some(request.requested_rows);
        }

        if self.complete || self.lines.len() >= request.requested_rows {
            return None;
        }

        Some(next_diff_cache_target(
            self.lines.len(),
            request.requested_rows,
        ))
    }

    fn matches_render_request(&self, request: DiffRenderRequest<'_>) -> bool {
        self.matches_file(request.file_id)
            && self.content_width == request.content_width
            && self.syntax_palette == request.syntax_palette
            && self.can_stage == request.can_stage
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    fn visible_lines(&self, scroll: usize, visible_height: usize) -> Vec<Line<'static>> {
        self.lines
            .iter()
            .skip(scroll)
            .take(visible_height)
            .cloned()
            .collect()
    }
}

impl DiffLayoutMetrics {
    pub fn new(total_rows: usize, hunk_offsets: Vec<usize>) -> Self {
        Self {
            total_rows,
            hunk_offsets,
        }
    }
}

impl CachedDiffLayoutMetrics {
    fn new(request: DiffLayoutRequest<'_>, metrics: DiffLayoutMetrics) -> Self {
        Self {
            file_id: request.file_id.to_string(),
            content_width: request.content_width,
            can_stage: request.can_stage,
            metrics,
        }
    }

    fn matches(&self, request: DiffLayoutRequest<'_>) -> bool {
        self.file_id == request.file_id
            && self.content_width == request.content_width
            && self.can_stage == request.can_stage
    }

    fn matches_file(&self, file_id: &str) -> bool {
        self.file_id == file_id
    }
}

fn next_diff_cache_target(current_rows: usize, requested_rows: usize) -> usize {
    requested_rows
        .max(current_rows.saturating_mul(2))
        .max(current_rows.saturating_add(1))
}

impl SidebarRowCountsCache {
    fn matches(&self, content_width: usize, can_stage: bool, file_count: usize) -> bool {
        self.content_width == content_width
            && self.can_stage == can_stage
            && self.row_counts.len() == file_count
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
    use crate::theme::Theme;

    #[test]
    fn clamped_scrolls_use_complete_rendered_diff_count() {
        let mut viewport = RenderedViewport::new(1);
        viewport.begin_diff(Rect::default(), 3);
        viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "file".to_string(),
                80,
                Theme::github_dark().syntax,
                true,
                vec![Line::raw("row"); 8],
                true,
            ),
        );

        let scrolls = viewport.clamped_scrolls(ViewportScrollInput {
            diff_scroll: 99,
            sidebar_scroll: 0,
            selected_file_index: 0,
            file_count: 1,
            selected_file_id: Some("file"),
            selected_file_line_count: 24,
        });

        assert_eq!(scrolls.diff_scroll, 5);
    }

    #[test]
    fn clamped_scrolls_keep_selected_sidebar_file_visible() {
        let mut viewport = RenderedViewport::new(6);
        viewport.begin_sidebar(Rect::default(), 3);

        let scrolls = viewport.clamped_scrolls(ViewportScrollInput {
            diff_scroll: 0,
            sidebar_scroll: 0,
            selected_file_index: 4,
            file_count: 6,
            selected_file_id: None,
            selected_file_line_count: 0,
        });
        assert_eq!(scrolls.sidebar_scroll, 2);

        let scrolls = viewport.clamped_scrolls(ViewportScrollInput {
            diff_scroll: 0,
            sidebar_scroll: 3,
            selected_file_index: 1,
            file_count: 6,
            selected_file_id: None,
            selected_file_line_count: 0,
        });
        assert_eq!(scrolls.sidebar_scroll, 1);
    }

    #[test]
    fn sidebar_row_mapping_records_visible_rows() {
        let mut viewport = RenderedViewport::new(4);
        viewport.begin_sidebar(Rect::new(0, 0, 12, 5), 3);
        viewport.record_sidebar_rows(2, 3);

        assert_eq!(viewport.sidebar_index_at(1, 1, 4), Some(2));
        assert_eq!(viewport.sidebar_index_at(1, 3, 4), Some(2));
        assert_eq!(viewport.sidebar_index_at(1, 4, 4), None);
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

    #[test]
    fn diff_lines_render_target_grows_partial_cache_geometrically() {
        let mut viewport = RenderedViewport::new(1);
        let theme = Theme::github_dark();
        viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "file".to_string(),
                80,
                theme.syntax,
                true,
                vec![Line::raw("row"); 100],
                false,
            ),
        );

        assert_eq!(
            viewport.diff_lines_render_target(DiffRenderRequest {
                file_index: 0,
                file_id: "file",
                content_width: 80,
                syntax_palette: theme.syntax,
                can_stage: true,
                requested_rows: 101,
            }),
            Some(200)
        );
        assert_eq!(
            viewport.diff_lines_render_target(DiffRenderRequest {
                file_index: 0,
                file_id: "file",
                content_width: 80,
                syntax_palette: theme.syntax,
                can_stage: true,
                requested_rows: 250,
            }),
            Some(250)
        );
    }
}
