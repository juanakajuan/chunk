//! Composition of the diff pane's rendered frame.
//!
//! `App` owns review state; this module owns the *ordering* that turns that
//! state into what the diff pane shows. One call — `diff_pane_rows` — stacks the
//! live-status rows above the diff, runs the two-pass search scroll
//! (render-and-locate matches, then scroll the active match into view), derives
//! the content-width and scrollbar geometry, renders the visible diff rows, and
//! decorates them for text selection. It is the single home for "what shows in
//! the diff pane".
//!
//! Persistent render geometry produced here — the status-row count and the
//! scrollbar — is stored back in `viewport` so hit-testing can read it between
//! frames without it leaking onto `App`.

use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::rows;
use crate::theme::Theme;
use crate::viewport::{DiffLayoutMetrics, DiffLayoutRequest, DiffRenderRequest, DiffScrollbar};

use super::{App, DiffPaneRows, pane_text_area, saturating_u16};

impl App {
    pub(crate) fn diff_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        if self.ask_ai_output().is_some() {
            return self.ask_ai_output_pane_rows(area, content_width, visible_height, theme);
        }
        if self.command_output().is_some() {
            return self.command_output_pane_rows(area, content_width, visible_height, theme);
        }

        let title = format!(" {} ", rows::changeset_title(&self.changeset));
        let mut lines = rows::live_status_lines(self.live_error.as_deref(), content_width, theme);
        let running = self.command_running();
        lines.extend(rows::custom_command_running_lines(
            running.map(|(binding, _)| binding),
            running.map_or(0, |(_, frame)| frame),
            content_width,
            theme,
        ));
        lines.extend(rows::ask_ai_prompt_lines(
            self.ask_ai_prompt().map(|prompt| prompt.input.as_str()),
            content_width,
            theme,
        ));
        let ask_ai_running = self.ask_ai_running();
        lines.extend(rows::ask_ai_running_lines(
            ask_ai_running.map(|(question, _, _)| question),
            ask_ai_running.map_or(0, |(_, frame, _)| frame),
            ask_ai_running.is_some_and(|(_, _, cancelling)| cancelling),
            content_width,
            theme,
        ));
        lines.extend(self.discard_status_lines(content_width, theme));

        let provisional_search_lines = self.search_status_lines(content_width, theme);
        let provisional_visible_diff_height =
            visible_height.saturating_sub(lines.len() + provisional_search_lines.len());
        let mut diff_content_width =
            self.diff_content_width(content_width, provisional_visible_diff_height, theme);

        let pending_search_scroll = if self.has_active_search() {
            self.viewport
                .begin_diff(area, provisional_visible_diff_height);
            self.ensure_scroll_bounds();
            self.ensure_selected_diff_cache(
                diff_content_width,
                provisional_visible_diff_height,
                theme,
            )
        } else {
            false
        };

        lines.extend(self.search_status_lines(content_width, theme));

        self.viewport.set_diff_status_rows(lines.len());
        let status_rows = self.viewport.diff_status_rows();
        let visible_diff_height = visible_height.saturating_sub(status_rows);
        diff_content_width = self.diff_content_width(content_width, visible_diff_height, theme);
        let total_diff_rows = self.selected_diff_line_count(diff_content_width, theme);
        self.viewport.begin_diff(area, visible_diff_height);
        let scrollbar = self.diff_scrollbar(
            area,
            content_width,
            visible_diff_height,
            total_diff_rows,
            status_rows,
        );
        self.viewport.set_diff_scrollbar(scrollbar);
        self.ensure_scroll_bounds();

        if pending_search_scroll {
            self.scroll_active_search_match();
            self.select_hunk_at_scroll();
            self.ensure_scroll_bounds();
        }

        if visible_diff_height > 0 {
            lines.extend(self.selected_diff_lines(diff_content_width, visible_diff_height, theme));
        }
        lines.truncate(visible_height);
        let scrollbar = self.diff_scrollbar(
            area,
            content_width,
            visible_diff_height,
            total_diff_rows,
            status_rows,
        );
        self.viewport.set_diff_scrollbar(scrollbar);

        let lines = self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: self.viewport.diff_scrollbar().cloned(),
        }
    }

    fn diff_content_width(
        &mut self,
        content_width: usize,
        visible_diff_height: usize,
        theme: Theme,
    ) -> usize {
        if content_width > 1
            && visible_diff_height > 0
            && self.selected_diff_line_count(content_width, theme) > visible_diff_height
        {
            content_width - 1
        } else {
            content_width
        }
    }

    fn selected_diff_line_count(&mut self, content_width: usize, theme: Theme) -> usize {
        self.selected_diff_layout_metrics(content_width, theme)
            .map_or(0, |metrics| metrics.total_rows)
    }

    fn selected_diff_layout_metrics(
        &mut self,
        content_width: usize,
        theme: Theme,
    ) -> Option<&DiffLayoutMetrics> {
        self.viewport
            .ensure_diff_cache_len(self.changeset.files.len());

        let selected_file_index = self.selected_file_index;
        let file_id = self.changeset.files.get(selected_file_index)?.id.clone();
        let request = DiffLayoutRequest {
            file_index: selected_file_index,
            file_id: file_id.as_str(),
            content_width,
            can_stage: self.can_stage(),
        };

        if self.viewport.diff_layout_metrics(request).is_none() {
            let file = &self.changeset.files[selected_file_index];
            let counts = rows::diff_layout_counts(file, content_width, theme, request.can_stage);
            self.viewport.cache_diff_layout_metrics(
                request,
                DiffLayoutMetrics::new(counts.total_rows, counts.hunk_offsets),
            );
        }

        self.viewport.diff_layout_metrics(request)
    }

    fn diff_scrollbar(
        &self,
        area: Rect,
        content_width: usize,
        visible_diff_height: usize,
        total_diff_rows: usize,
        status_rows: usize,
    ) -> Option<DiffScrollbar> {
        if content_width <= 1 || visible_diff_height == 0 || total_diff_rows <= visible_diff_height
        {
            return None;
        }

        let file = self.selected_file()?;
        let scrollbar_area = Rect {
            x: area.x.saturating_add(area.width.saturating_sub(2)),
            y: area
                .y
                .saturating_add(1)
                .saturating_add(saturating_u16(status_rows)),
            width: 1,
            height: saturating_u16(visible_diff_height),
        };

        Some(DiffScrollbar::new(
            scrollbar_area,
            self.selected_file_index,
            file.id.clone(),
            total_diff_rows,
            visible_diff_height,
            self.diff_scroll,
        ))
    }

    fn selected_diff_lines(
        &mut self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        self.ensure_selected_diff_cache(content_width, visible_height, theme);
        self.visible_selected_diff_lines(content_width, visible_height, theme)
    }

    fn ensure_selected_diff_cache(
        &mut self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> bool {
        self.viewport
            .ensure_diff_cache_len(self.changeset.files.len());

        let selected_file_index = self.selected_file_index;
        let can_stage = self.can_stage();
        if selected_file_index >= self.changeset.files.len() {
            self.clear_rendered_search_matches();
            return false;
        }

        let target_rows = self.diff_render_target_rows(visible_height);
        let hunk_offsets = self
            .selected_diff_layout_metrics(content_width, theme)
            .map(|metrics| metrics.hunk_offsets.clone())
            .unwrap_or_default();

        let file_id = self.changeset.files[selected_file_index].id.clone();
        let request = DiffRenderRequest {
            file_index: selected_file_index,
            file_id: file_id.as_str(),
            content_width,
            syntax_palette: theme.syntax,
            can_stage,
            requested_rows: target_rows,
        };

        // Split borrows so the render seam can load source snapshots and render
        // rows while the viewport owns the cache; source snapshots load only
        // when the viewport actually invokes `render`.
        let Self {
            viewport,
            changeset,
            source,
            ..
        } = self;
        viewport.ensure_diff_lines(request, hunk_offsets, |render_target| {
            if let Some(file) = changeset.files.get_mut(selected_file_index) {
                source.load_source_snapshots(file);
            }
            let file = &changeset.files[selected_file_index];
            let rendered =
                rows::diff_lines_until(file, content_width, theme, can_stage, None, render_target);
            (rendered.lines, rendered.complete)
        });

        let pending_search_scroll = self.refresh_search_matches(selected_file_index);
        self.ensure_scroll_bounds();

        pending_search_scroll
    }

    fn visible_selected_diff_lines(
        &self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        if self.selected_file_index >= self.changeset.files.len() {
            return rows::no_diff_lines(self.no_diff_message(), content_width, theme);
        }

        let mut lines = self.viewport.visible_diff_lines(
            self.selected_file_index,
            self.diff_scroll,
            visible_height,
        );
        self.apply_selected_hunk_style(&mut lines, content_width, theme);
        self.highlight_search_matches(lines, theme)
    }

    fn command_output_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        self.viewport.begin_diff(area, visible_height);
        self.viewport.set_diff_status_rows(0);

        let output = self
            .command_output_mut()
            .expect("command output pane requires command output state");
        let title = format!(" Command: {} ", output.result.label());
        let all_lines = rows::custom_command_output_lines(&output.result, content_width, theme);
        output.scroll.sync(all_lines.len(), visible_height);
        let lines = output.scroll.visible(all_lines);
        let lines = self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: None,
        }
    }

    fn ask_ai_output_pane_rows(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> DiffPaneRows {
        self.viewport.begin_diff(area, visible_height);
        self.viewport.set_diff_status_rows(0);

        let output = self
            .ask_ai_output_mut()
            .expect("Ask AI output pane requires output state");
        let title = format!(" Ask AI: {} ", output.result.context_summary());
        let all_lines = rows::ask_ai_output_lines(&output.result, content_width, theme);
        output.scroll.sync(all_lines.len(), visible_height);
        let lines = output.scroll.visible(all_lines);
        let lines = self.text_selection.decorate_visible_lines(
            pane_text_area(area, content_width, visible_height),
            lines,
            0,
            visible_height,
            theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: None,
        }
    }

    fn apply_selected_hunk_style(
        &self,
        lines: &mut [Line<'static>],
        content_width: usize,
        theme: Theme,
    ) {
        let Some(selected_hunk_index) = self.selected_hunk_index else {
            return;
        };
        let Some(file) = self.selected_file() else {
            return;
        };
        let Some(hunk) = file.hunks.get(selected_hunk_index) else {
            return;
        };
        let Some(hunk_offset) = self.viewport.hunk_offset(
            self.selected_file_index,
            file.id.as_str(),
            selected_hunk_index,
        ) else {
            return;
        };

        let visible_start = self.diff_scroll;
        let visible_end = visible_start.saturating_add(lines.len());
        let header_rows =
            rows::selected_hunk_header_rows(hunk, content_width, theme, self.can_stage());
        for (header_row_offset, header_row) in header_rows.into_iter().enumerate() {
            let rendered_row = hunk_offset.saturating_add(header_row_offset);
            if rendered_row < visible_start || rendered_row >= visible_end {
                continue;
            }

            lines[rendered_row - visible_start] = header_row;
        }
    }
}
