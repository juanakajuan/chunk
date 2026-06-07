//! Rendered viewport state for terminal panes.
//!
//! This module owns geometry, row mapping, and render caches. `App` owns review
//! state; `ui` owns drawing. Mouse hit testing and scroll bounds meet here.

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
    /// Rendered sidebar row to file index mapping for click handling.
    sidebar_row_indices: Vec<usize>,
    /// Cached sidebar row counts for the current sidebar layout.
    sidebar_row_counts_cache: Option<SidebarRowCountsCache>,
    /// Cached wrapped and highlighted diff lines by file index.
    diff_lines_cache: Vec<Option<RenderedDiffLines>>,
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
    /// Whether `lines` contains every rendered row for the file.
    complete: bool,
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
            sidebar_row_indices: Vec::new(),
            sidebar_row_counts_cache: None,
            diff_lines_cache: vec![None; file_count],
        }
    }

    pub fn begin_frame(&mut self) {
        self.sidebar_area = None;
        self.diff_area = None;
        self.sidebar_row_indices.clear();
    }

    pub fn clear_render_caches(&mut self, file_count: usize) {
        self.sidebar_row_counts_cache = None;
        self.diff_lines_cache = vec![None; file_count];
    }

    pub fn diff_view_height(&self) -> usize {
        self.diff_view_height
    }

    pub fn sidebar_view_height(&self) -> usize {
        self.sidebar_view_height
    }

    pub fn set_diff_view_height(&mut self, height: usize) {
        self.diff_view_height = height.max(1);
    }

    pub fn set_sidebar_view_height(&mut self, height: usize) {
        self.sidebar_view_height = height.max(1);
    }

    pub fn set_sidebar_area(&mut self, area: Rect) {
        self.sidebar_area = Some(area);
    }

    pub fn set_diff_area(&mut self, area: Rect) {
        self.diff_area = Some(area);
    }

    pub fn clear_sidebar_rows(&mut self) {
        self.sidebar_row_indices.clear();
    }

    pub fn push_sidebar_row_index(&mut self, index: usize) {
        self.sidebar_row_indices.push(index);
    }

    #[cfg(test)]
    pub fn sidebar_row_indices(&self) -> &[usize] {
        &self.sidebar_row_indices
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

    pub fn rendered_diff_line_count(
        &self,
        file_index: usize,
        file_id: &str,
        fallback: usize,
    ) -> usize {
        let Some(cache) = self.diff_lines_cache(file_index) else {
            return fallback;
        };

        if !cache.matches_file(file_id) {
            return fallback;
        }

        if cache.complete {
            cache.len()
        } else {
            cache.len().max(fallback)
        }
    }

    pub fn ensure_diff_lines_cache_len(&mut self, file_count: usize) {
        if self.diff_lines_cache.len() != file_count {
            self.diff_lines_cache = vec![None; file_count];
        }
    }

    pub fn diff_lines_need_render(
        &self,
        file_index: usize,
        file_id: &str,
        content_width: usize,
        syntax_palette: SyntaxPalette,
        can_stage: bool,
        target_rows: usize,
    ) -> bool {
        self.diff_lines_cache(file_index).is_none_or(|cache| {
            !cache.is_valid_for(
                file_id,
                content_width,
                syntax_palette,
                can_stage,
                target_rows,
            )
        })
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
            complete,
        }
    }

    fn matches_file(&self, file_id: &str) -> bool {
        self.file_id == file_id
    }

    fn is_valid_for(
        &self,
        file_id: &str,
        content_width: usize,
        syntax_palette: SyntaxPalette,
        can_stage: bool,
        target_rows: usize,
    ) -> bool {
        self.matches_file(file_id)
            && self.content_width == content_width
            && self.syntax_palette == syntax_palette
            && self.can_stage == can_stage
            && (self.complete || self.lines.len() >= target_rows)
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
