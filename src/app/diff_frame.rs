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
        DiffFrameRenderer::new(
            self,
            DiffFrameRequest {
                area,
                content_width,
                visible_height,
                theme,
            },
        )
        .render()
    }
}

#[derive(Debug, Clone, Copy)]
struct DiffFrameRequest {
    area: Rect,
    content_width: usize,
    visible_height: usize,
    theme: Theme,
}

struct DiffFrameRenderer<'a> {
    app: &'a mut App,
    request: DiffFrameRequest,
}

impl<'a> DiffFrameRenderer<'a> {
    fn new(app: &'a mut App, request: DiffFrameRequest) -> Self {
        Self { app, request }
    }

    fn render(mut self) -> DiffPaneRows {
        if self.app.ask_ai_output().is_some() {
            return self.ask_ai_output_pane_rows();
        }
        if self.app.command_output().is_some() {
            return self.command_output_pane_rows();
        }

        self.review_pane_rows()
    }

    fn review_pane_rows(&mut self) -> DiffPaneRows {
        let title = format!(" {} ", rows::changeset_title(&self.app.changeset));
        let mut lines = self.review_status_lines();

        let provisional_search_lines = self
            .app
            .search_status_lines(self.request.content_width, self.request.theme);
        let provisional_visible_diff_height = self
            .request
            .visible_height
            .saturating_sub(lines.len() + provisional_search_lines.len());
        let mut diff_content_width = self.diff_content_width(provisional_visible_diff_height);

        let pending_search_scroll = if self.app.has_active_search() {
            self.app
                .viewport
                .begin_diff(self.request.area, provisional_visible_diff_height);
            self.app.ensure_scroll_bounds();
            self.ensure_selected_diff_cache(diff_content_width, provisional_visible_diff_height)
        } else {
            false
        };

        lines.extend(
            self.app
                .search_status_lines(self.request.content_width, self.request.theme),
        );

        self.app.viewport.set_diff_status_rows(lines.len());
        let status_rows = self.app.viewport.diff_status_rows();
        let visible_diff_height = self.request.visible_height.saturating_sub(status_rows);
        diff_content_width = self.diff_content_width(visible_diff_height);
        let total_diff_rows = self.selected_diff_line_count(diff_content_width);
        self.app
            .viewport
            .begin_diff(self.request.area, visible_diff_height);
        let scrollbar = self.diff_scrollbar(visible_diff_height, total_diff_rows, status_rows);
        self.app.viewport.set_diff_scrollbar(scrollbar);
        self.app.ensure_scroll_bounds();

        if pending_search_scroll {
            self.app.diff_pane.scroll_active_search_match(
                &self.app.viewport,
                self.app.selected_file_index,
                self.app.changeset.files.get(self.app.selected_file_index),
            );
            self.app.ensure_scroll_bounds();
        }

        if visible_diff_height > 0 {
            lines.extend(self.selected_diff_lines(diff_content_width, visible_diff_height));
        }
        lines.truncate(self.request.visible_height);
        let scrollbar = self.diff_scrollbar(visible_diff_height, total_diff_rows, status_rows);
        self.app.viewport.set_diff_scrollbar(scrollbar);

        let lines = self.app.text_selection.decorate_visible_lines(
            pane_text_area(
                self.request.area,
                self.request.content_width,
                self.request.visible_height,
            ),
            lines,
            0,
            self.request.visible_height,
            self.request.theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: self.app.viewport.diff_scrollbar().cloned(),
        }
    }

    fn review_status_lines(&self) -> Vec<Line<'static>> {
        let mut lines = rows::live_status_lines(
            self.app.live_error.as_deref(),
            self.app.live_notice.as_deref(),
            self.request.content_width,
            self.request.theme,
        );
        let running = self.app.command_running();
        lines.extend(rows::custom_command_running_lines(
            running.map(|(binding, _, _)| binding),
            running.map_or(0, |(_, frame, _)| frame),
            running.is_some_and(|(_, _, cancelling)| cancelling),
            self.request.content_width,
            self.request.theme,
        ));
        lines.extend(rows::ask_ai_prompt_lines(
            self.app.ask_ai_prompt().map(|prompt| prompt.input.as_str()),
            self.request.content_width,
            self.request.theme,
        ));
        let ask_ai_running = self.app.ask_ai_running();
        lines.extend(rows::ask_ai_running_lines(
            ask_ai_running.map(|(question, _, _)| question),
            ask_ai_running.map_or(0, |(_, frame, _)| frame),
            ask_ai_running.is_some_and(|(_, _, cancelling)| cancelling),
            self.request.content_width,
            self.request.theme,
        ));
        lines.extend(
            self.app
                .discard_status_lines(self.request.content_width, self.request.theme),
        );

        lines
    }

    fn diff_content_width(&mut self, visible_diff_height: usize) -> usize {
        if self.request.content_width > 1
            && visible_diff_height > 0
            && self.selected_diff_line_count(self.request.content_width) > visible_diff_height
        {
            self.request.content_width - 1
        } else {
            self.request.content_width
        }
    }

    fn selected_diff_line_count(&mut self, content_width: usize) -> usize {
        self.selected_diff_layout_metrics(content_width)
            .map_or(0, |metrics| metrics.total_rows)
    }

    fn selected_diff_layout_metrics(&mut self, content_width: usize) -> Option<&DiffLayoutMetrics> {
        self.app
            .viewport
            .ensure_diff_cache_len(self.app.changeset.files.len());

        let selected_file_index = self.app.selected_file_index;
        let file_id = self
            .app
            .changeset
            .files
            .get(selected_file_index)?
            .id
            .clone();
        let request = DiffLayoutRequest {
            file_index: selected_file_index,
            file_id: file_id.as_str(),
            content_width,
            can_stage: self.app.can_stage(),
        };

        if self.app.viewport.diff_layout_metrics(request).is_none() {
            let file = &self.app.changeset.files[selected_file_index];
            let counts = rows::diff_layout_counts(
                file,
                content_width,
                self.request.theme,
                request.can_stage,
            );
            self.app.viewport.cache_diff_layout_metrics(
                request,
                DiffLayoutMetrics::new(counts.hunk_offsets, counts.new_line_rows),
            );
        }

        self.app.viewport.diff_layout_metrics(request)
    }

    fn diff_scrollbar(
        &self,
        visible_diff_height: usize,
        total_diff_rows: usize,
        status_rows: usize,
    ) -> Option<DiffScrollbar> {
        if self.request.content_width <= 1
            || visible_diff_height == 0
            || total_diff_rows <= visible_diff_height
        {
            return None;
        }

        let file = self.app.selected_file()?;
        let scrollbar_area = Rect {
            x: self
                .request
                .area
                .x
                .saturating_add(self.request.area.width.saturating_sub(2)),
            y: self
                .request
                .area
                .y
                .saturating_add(1)
                .saturating_add(saturating_u16(status_rows)),
            width: 1,
            height: saturating_u16(visible_diff_height),
        };

        Some(DiffScrollbar::new(
            scrollbar_area,
            self.app.selected_file_index,
            file.id.clone(),
            total_diff_rows,
            visible_diff_height,
            self.app.diff_pane.scroll(),
        ))
    }

    fn selected_diff_lines(
        &mut self,
        content_width: usize,
        visible_height: usize,
    ) -> Vec<Line<'static>> {
        self.ensure_selected_diff_cache(content_width, visible_height);
        self.visible_selected_diff_lines(content_width, visible_height)
    }

    fn ensure_selected_diff_cache(&mut self, content_width: usize, visible_height: usize) -> bool {
        self.app
            .viewport
            .ensure_diff_cache_len(self.app.changeset.files.len());

        let selected_file_index = self.app.selected_file_index;
        let can_stage = self.app.can_stage();
        if selected_file_index >= self.app.changeset.files.len() {
            self.app.clear_rendered_search_matches();
            return false;
        }

        let target_rows = self.app.diff_render_target_rows(visible_height);
        let hunk_offsets = self
            .selected_diff_layout_metrics(content_width)
            .map(|metrics| metrics.hunk_offsets.clone())
            .unwrap_or_default();

        let file_id = self.app.changeset.files[selected_file_index].id.clone();
        let request = DiffRenderRequest {
            file_index: selected_file_index,
            file_id: file_id.as_str(),
            content_width,
            syntax_palette: self.request.theme.syntax,
            can_stage,
            requested_rows: target_rows,
        };

        // Split borrows so the render seam can load source snapshots and render
        // rows while the viewport owns the cache; source snapshots load only
        // when the viewport actually invokes `render`.
        {
            let App {
                viewport,
                changeset,
                source,
                ..
            } = &mut *self.app;
            viewport.ensure_diff_lines(request, hunk_offsets, |render_target| {
                if let Some(file) = changeset.files.get_mut(selected_file_index) {
                    source.load_source_snapshots(file);
                }
                let file = &changeset.files[selected_file_index];
                let rendered = rows::diff_lines_until(
                    file,
                    content_width,
                    self.request.theme,
                    can_stage,
                    None,
                    render_target,
                );
                (rendered.lines, rendered.complete)
            });
        }

        let pending_search_scroll = self.app.refresh_search_matches(selected_file_index);
        self.app.ensure_scroll_bounds();

        pending_search_scroll
    }

    fn visible_selected_diff_lines(
        &self,
        content_width: usize,
        visible_height: usize,
    ) -> Vec<Line<'static>> {
        if self.app.selected_file_index >= self.app.changeset.files.len() {
            return rows::no_diff_lines(
                self.app.no_diff_message(),
                content_width,
                self.request.theme,
            );
        }

        let mut lines = self.app.viewport.visible_diff_lines(
            self.app.selected_file_index,
            self.app.diff_pane.scroll(),
            visible_height,
        );
        self.apply_selected_hunk_style(&mut lines, content_width);
        self.app.highlight_search_matches(lines, self.request.theme)
    }

    fn command_output_pane_rows(&mut self) -> DiffPaneRows {
        let content_width = self.request.content_width;
        let theme = self.request.theme;
        self.output_pane_rows(|app, output_height| {
            let output = app
                .command_output_mut()
                .expect("command output pane requires command output state");
            let title = format!(" Command: {} ", output.result.label());
            let all_lines = rows::custom_command_output_lines(&output.result, content_width, theme);
            output.scroll.sync(all_lines.len(), output_height);

            (title, output.scroll.visible(all_lines))
        })
    }

    fn ask_ai_output_pane_rows(&mut self) -> DiffPaneRows {
        let content_width = self.request.content_width;
        let theme = self.request.theme;
        self.output_pane_rows(|app, output_height| {
            let output = app
                .ask_ai_output_mut()
                .expect("Ask AI output pane requires output state");
            let title = format!(" Ask AI: {} ", output.result.context_summary());
            let all_lines = rows::ask_ai_output_lines(&output.result, content_width, theme);
            output.scroll.sync(all_lines.len(), output_height);

            (title, output.scroll.visible(all_lines))
        })
    }

    fn output_pane_rows(
        &mut self,
        pane_lines: impl FnOnce(&mut App, usize) -> (String, Vec<Line<'static>>),
    ) -> DiffPaneRows {
        let mut lines = rows::live_status_lines(
            self.app.live_error.as_deref(),
            self.app.live_notice.as_deref(),
            self.request.content_width,
            self.request.theme,
        );
        let output_height = self.request.visible_height.saturating_sub(lines.len());

        self.app
            .viewport
            .begin_diff(self.request.area, self.request.visible_height);
        self.app.viewport.set_diff_status_rows(lines.len());

        let (title, visible_output_lines) = pane_lines(self.app, output_height);
        lines.extend(visible_output_lines);
        lines.truncate(self.request.visible_height);
        let lines = self.app.text_selection.decorate_visible_lines(
            pane_text_area(
                self.request.area,
                self.request.content_width,
                self.request.visible_height,
            ),
            lines,
            0,
            self.request.visible_height,
            self.request.theme,
        );

        DiffPaneRows {
            title,
            lines,
            scrollbar: None,
        }
    }

    fn apply_selected_hunk_style(&self, lines: &mut [Line<'static>], content_width: usize) {
        let Some(selected_hunk_index) = self.app.diff_pane.selected_hunk_index() else {
            return;
        };
        let Some(file) = self.app.selected_file() else {
            return;
        };
        let Some(hunk) = file.hunks.get(selected_hunk_index) else {
            return;
        };
        let Some(hunk_offset) = self.app.viewport.hunk_offset(
            self.app.selected_file_index,
            file.id.as_str(),
            selected_hunk_index,
        ) else {
            return;
        };

        let visible_start = self.app.diff_pane.scroll();
        let visible_end = visible_start.saturating_add(lines.len());
        let header_rows = rows::selected_hunk_header_rows(
            hunk,
            content_width,
            self.request.theme,
            self.app.can_stage(),
        );
        for (header_row_offset, header_row) in header_rows.into_iter().enumerate() {
            let rendered_row = hunk_offset.saturating_add(header_row_offset);
            if rendered_row < visible_start || rendered_row >= visible_end {
                continue;
            }

            lines[rendered_row - visible_start] = header_row;
        }
    }
}
