//! Rendered diff rows, layout metrics, and render cache state.
//!
//! This module owns the wrapped-row representation of a selected diff file:
//! cache growth, layout metrics, source snapshot loading, hunk row lookup,
//! editor-line lookup, and feeding rendered rows into search. `viewport` owns
//! terminal geometry and hit-testing; `rows` owns the formatting of individual
//! rows.

use ratatui::text::Line;

use crate::model::{Changeset, DiffFile};
use crate::review_source::ReviewSource;
use crate::rows;
use crate::search::Search;
use crate::theme::{SyntaxPalette, Theme};

#[derive(Debug)]
pub(crate) struct DiffRenderState {
    /// Cached wrapped and highlighted diff lines by file index.
    diff_lines_cache: Vec<Option<RenderedDiffLines>>,
    /// Cached full diff row metrics by file index.
    diff_layout_cache: Vec<Option<CachedDiffLayoutMetrics>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SelectedDiffRenderRequest {
    pub(crate) file_index: usize,
    pub(crate) content_width: usize,
    pub(crate) theme: Theme,
    pub(crate) can_stage: bool,
    pub(crate) requested_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffRenderRequest<'a> {
    file_index: usize,
    file_id: &'a str,
    content_width: usize,
    syntax_palette: SyntaxPalette,
    can_stage: bool,
    requested_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffLayoutRequest<'a> {
    file_index: usize,
    file_id: &'a str,
    content_width: usize,
    can_stage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffLayoutMetrics {
    total_rows: usize,
    hunk_offsets: Vec<usize>,
    new_line_rows: Vec<Option<u32>>,
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedDiffLines {
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

impl DiffRenderState {
    pub(crate) fn new(file_count: usize) -> Self {
        Self {
            diff_lines_cache: vec![None; file_count],
            diff_layout_cache: vec![None; file_count],
        }
    }

    pub(crate) fn clear(&mut self, file_count: usize) {
        self.diff_lines_cache = vec![None; file_count];
        self.diff_layout_cache = vec![None; file_count];
    }

    pub(crate) fn total_rows(
        &mut self,
        file_index: usize,
        file: Option<&DiffFile>,
        content_width: usize,
        theme: Theme,
        can_stage: bool,
    ) -> usize {
        let Some(file) = file else {
            return 0;
        };

        let request = DiffLayoutRequest {
            file_index,
            file_id: file.id.as_str(),
            content_width,
            can_stage,
        };
        self.layout_metrics(file, request, theme)
            .map_or(0, |metrics| metrics.total_rows)
    }

    pub(crate) fn max_scroll(
        &self,
        file_index: usize,
        file_id: Option<&str>,
        fallback_line_count: usize,
        view_height: usize,
    ) -> usize {
        let rendered_scroll = self
            .rendered_diff_line_count(file_index, file_id, fallback_line_count)
            .saturating_sub(view_height);
        let layout_scroll = self
            .matching_diff_layout_cache(file_index, file_id)
            .map(|cache| cache.metrics.total_rows.saturating_sub(view_height))
            .unwrap_or(0);
        let hunk_scroll = self.partial_cache_hunk_scroll_target(file_index, file_id);

        rendered_scroll.max(layout_scroll).max(hunk_scroll)
    }

    pub(crate) fn ensure_selected_file(
        &mut self,
        changeset: &mut Changeset,
        source: &ReviewSource,
        request: SelectedDiffRenderRequest,
    ) {
        self.ensure_cache_len(changeset.files.len());
        let Some(file_id) = changeset
            .files
            .get(request.file_index)
            .map(|file| file.id.clone())
        else {
            return;
        };

        let layout_request = DiffLayoutRequest {
            file_index: request.file_index,
            file_id: file_id.as_str(),
            content_width: request.content_width,
            can_stage: request.can_stage,
        };
        let hunk_offsets = self
            .layout_metrics(
                &changeset.files[request.file_index],
                layout_request,
                request.theme,
            )
            .map(|metrics| metrics.hunk_offsets.clone())
            .unwrap_or_default();

        let render_request = DiffRenderRequest {
            file_index: request.file_index,
            file_id: file_id.as_str(),
            content_width: request.content_width,
            syntax_palette: request.theme.syntax,
            can_stage: request.can_stage,
            requested_rows: request.requested_rows,
        };
        let Some(render_target) = self.diff_lines_render_target(render_request) else {
            return;
        };

        if let Some(file) = changeset.files.get_mut(request.file_index) {
            source.load_source_snapshots(file);
        }

        let rendered = rows::diff_lines_until(
            &changeset.files[request.file_index],
            request.content_width,
            request.theme,
            request.can_stage,
            None,
            render_target,
        );
        self.cache_diff_lines(
            render_request,
            RenderedDiffLines::new(
                file_id.clone(),
                request.content_width,
                request.theme.syntax,
                request.can_stage,
                rendered.lines,
                rendered.complete,
            )
            .with_hunk_offsets(hunk_offsets),
        );
    }

    pub(crate) fn refresh_search_matches(
        &self,
        search: &mut Search,
        file_index: usize,
        file: Option<&DiffFile>,
    ) -> bool {
        let Some(file) = file else {
            search.clear_rendered_matches();
            return false;
        };

        let Some(lines) = self.diff_lines(file_index, file.id.as_str()) else {
            search.clear_rendered_matches();
            return false;
        };

        search.refresh_matches(file.id.as_str(), lines)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn visible_selected_lines(
        &self,
        file_index: usize,
        file: &DiffFile,
        scroll: usize,
        visible_height: usize,
        content_width: usize,
        theme: Theme,
        can_stage: bool,
        selected_hunk_index: Option<usize>,
    ) -> Vec<Line<'static>> {
        let mut lines = self.visible_lines(file_index, scroll, visible_height);
        self.apply_selected_hunk_style(
            &mut lines,
            file_index,
            file,
            scroll,
            content_width,
            theme,
            can_stage,
            selected_hunk_index,
        );
        lines
    }

    /// Rendered row offset of `hunk_index` for the cached file, if known.
    pub(crate) fn hunk_offset(
        &self,
        file_index: usize,
        file_id: &str,
        hunk_index: usize,
    ) -> Option<usize> {
        self.diff_hunk_offsets(file_index, file_id)
            .and_then(|offsets| offsets.get(hunk_index).copied())
    }

    /// Hunk index occupying `rendered_row`, mapping a scroll position back to a
    /// hunk via the cached offsets. Falls back to the first hunk when offsets
    /// are not yet cached.
    pub(crate) fn hunk_index_at(
        &self,
        file_index: usize,
        file_id: &str,
        rendered_row: usize,
        hunk_count: usize,
    ) -> Option<usize> {
        if hunk_count == 0 {
            return None;
        }

        let Some(offsets) = self
            .diff_hunk_offsets(file_index, file_id)
            .filter(|offsets| !offsets.is_empty())
        else {
            return Some(0);
        };

        Some(
            offsets
                .iter()
                .rposition(|offset| *offset <= rendered_row)
                .unwrap_or(0)
                .min(hunk_count - 1),
        )
    }

    /// New-file line corresponding to `rendered_row`, if layout metrics are
    /// cached for the selected file.
    pub(crate) fn new_line_at(
        &self,
        file_index: usize,
        file_id: &str,
        rendered_row: usize,
    ) -> Option<u32> {
        self.matching_diff_layout_cache(file_index, Some(file_id))
            .and_then(|cache| cache.metrics.new_line_at(rendered_row))
    }

    fn ensure_cache_len(&mut self, file_count: usize) {
        if self.diff_lines_cache.len() != file_count {
            self.diff_lines_cache = vec![None; file_count];
        }
        if self.diff_layout_cache.len() != file_count {
            self.diff_layout_cache = vec![None; file_count];
        }
    }

    fn ensure_cache_index(&mut self, file_index: usize) {
        let cache_len = file_index.saturating_add(1);
        if self.diff_lines_cache.len() < cache_len {
            self.diff_lines_cache.resize(cache_len, None);
        }
        if self.diff_layout_cache.len() < cache_len {
            self.diff_layout_cache.resize(cache_len, None);
        }
    }

    fn layout_metrics(
        &mut self,
        file: &DiffFile,
        request: DiffLayoutRequest<'_>,
        theme: Theme,
    ) -> Option<&DiffLayoutMetrics> {
        self.ensure_cache_index(request.file_index);
        if self.diff_layout_metrics(request).is_none() {
            let counts =
                rows::diff_layout_counts(file, request.content_width, theme, request.can_stage);
            self.cache_diff_layout_metrics(
                request,
                DiffLayoutMetrics::new(counts.hunk_offsets, counts.new_line_rows),
            );
        }

        self.diff_layout_metrics(request)
    }

    fn diff_layout_metrics(&self, request: DiffLayoutRequest<'_>) -> Option<&DiffLayoutMetrics> {
        self.diff_layout_cache(request.file_index)
            .filter(|cache| cache.matches(request))
            .map(|cache| &cache.metrics)
    }

    fn cache_diff_layout_metrics(
        &mut self,
        request: DiffLayoutRequest<'_>,
        metrics: DiffLayoutMetrics,
    ) {
        if let Some(cache_slot) = self.diff_layout_cache.get_mut(request.file_index) {
            *cache_slot = Some(CachedDiffLayoutMetrics::new(request, metrics));
        }
    }

    fn diff_lines_render_target(&self, request: DiffRenderRequest<'_>) -> Option<usize> {
        let Some(cache) = self.diff_lines_cache(request.file_index) else {
            return Some(request.requested_rows);
        };

        cache.render_target_if_needed(request)
    }

    fn cache_diff_lines(&mut self, request: DiffRenderRequest<'_>, lines: RenderedDiffLines) {
        if let Some(cache_slot) = self.diff_lines_cache.get_mut(request.file_index) {
            *cache_slot = Some(lines);
        }
    }

    #[cfg(test)]
    pub(crate) fn cache_test_diff_lines(&mut self, file_index: usize, lines: RenderedDiffLines) {
        if let Some(cache_slot) = self.diff_lines_cache.get_mut(file_index) {
            *cache_slot = Some(lines);
        }
    }

    fn visible_lines(
        &self,
        file_index: usize,
        scroll: usize,
        visible_height: usize,
    ) -> Vec<Line<'static>> {
        self.diff_lines_cache(file_index)
            .map(|cache| cache.visible_lines(scroll, visible_height))
            .unwrap_or_default()
    }

    fn diff_lines(&self, file_index: usize, file_id: &str) -> Option<&[Line<'static>]> {
        self.matching_diff_lines_cache(file_index, Some(file_id))
            .map(|cache| cache.lines.as_slice())
    }

    fn diff_hunk_offsets(&self, file_index: usize, file_id: &str) -> Option<&[usize]> {
        self.matching_diff_lines_cache(file_index, Some(file_id))
            .map(|cache| cache.hunk_offsets.as_slice())
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

    fn partial_cache_hunk_scroll_target(&self, file_index: usize, file_id: Option<&str>) -> usize {
        self.matching_diff_lines_cache(file_index, file_id)
            .filter(|cache| !cache.complete)
            .and_then(|cache| cache.hunk_offsets.last().copied())
            .unwrap_or(0)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_selected_hunk_style(
        &self,
        lines: &mut [Line<'static>],
        file_index: usize,
        file: &DiffFile,
        scroll: usize,
        content_width: usize,
        theme: Theme,
        can_stage: bool,
        selected_hunk_index: Option<usize>,
    ) {
        let Some(selected_hunk_index) = selected_hunk_index else {
            return;
        };
        let Some(hunk) = file.hunks.get(selected_hunk_index) else {
            return;
        };
        let Some(hunk_offset) = self.hunk_offset(file_index, file.id.as_str(), selected_hunk_index)
        else {
            return;
        };

        let visible_start = scroll;
        let visible_end = visible_start.saturating_add(lines.len());
        let header_rows = rows::selected_hunk_header_rows(hunk, content_width, theme, can_stage);
        for (header_row_offset, header_row) in header_rows.into_iter().enumerate() {
            let rendered_row = hunk_offset.saturating_add(header_row_offset);
            if rendered_row < visible_start || rendered_row >= visible_end {
                continue;
            }

            lines[rendered_row - visible_start] = header_row;
        }
    }
}

impl RenderedDiffLines {
    pub(crate) fn new(
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

    pub(crate) fn with_hunk_offsets(mut self, hunk_offsets: Vec<usize>) -> Self {
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
    fn new(hunk_offsets: Vec<usize>, new_line_rows: Vec<Option<u32>>) -> Self {
        Self {
            total_rows: new_line_rows.len(),
            hunk_offsets,
            new_line_rows,
        }
    }

    fn new_line_at(&self, rendered_row: usize) -> Option<u32> {
        self.new_line_rows.get(rendered_row).copied().flatten()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_target_grows_partial_cache_geometrically() {
        let mut render = DiffRenderState::new(1);
        let theme = Theme::github_dark();
        render.cache_test_diff_lines(
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
            render.diff_lines_render_target(DiffRenderRequest {
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
            render.diff_lines_render_target(DiffRenderRequest {
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
