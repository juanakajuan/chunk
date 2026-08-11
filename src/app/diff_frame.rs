//! Composition of the pane occupying the application's diff region.
//!
//! Output panes and text selection remain application concerns. Review-diff
//! rendering is delegated as one operation to `SelectedDiffPane`, which owns
//! every diff-local cache and piece of frame geometry.

use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::rows;
use crate::theme::Theme;

use super::selected_diff::SelectedReviewFrame;
use super::{App, DiffPaneRows, pane_text_area};

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
        // Callers outside the full UI render loop (notably tests) still get
        // frame-safe hit geometry.
        self.app.selected_diff.invalidate_frame();
        self.app.repository_review.invalidate_details_frame();
        if self.app.ask_ai_output().is_some() {
            return self.ask_ai_output_pane_rows();
        }
        if self.app.command_output().is_some() {
            return self.command_output_pane_rows();
        }

        if self.app.repository_review.details_active() {
            self.commit_details_pane_rows()
        } else {
            self.review_pane_rows()
        }
    }

    fn commit_details_pane_rows(&mut self) -> DiffPaneRows {
        let rendered = self.app.repository_review.render_details(
            self.request.area,
            self.request.content_width,
            self.request.visible_height,
            self.request.theme,
        );
        let lines = self.decorate_visible_lines(rendered.lines, rendered.line_scroll);
        DiffPaneRows {
            title: rendered.title,
            lines,
            scrollbar: None,
        }
    }

    fn review_pane_rows(&mut self) -> DiffPaneRows {
        let title = format!(" {} ", rows::changeset_title(&self.app.changeset));
        let leading_rows = self.review_status_lines();
        let no_diff_message = self.app.no_diff_message();
        let can_stage = self.app.can_stage();
        let selected_file_index = self.app.selected_file_index();
        let rendered = self.app.selected_diff.render(SelectedReviewFrame {
            area: self.request.area,
            content_width: self.request.content_width,
            visible_height: self.request.visible_height,
            theme: self.request.theme,
            can_stage,
            file_index: selected_file_index,
            changeset: &mut self.app.changeset,
            source: &self.app.source,
            no_diff_message,
            leading_rows,
        });
        let lines = self.decorate_visible_lines(rendered.lines, 0);

        DiffPaneRows {
            title,
            lines,
            scrollbar: rendered.scrollbar,
        }
    }

    fn decorate_visible_lines(
        &mut self,
        lines: Vec<Line<'static>>,
        line_scroll: usize,
    ) -> Vec<Line<'static>> {
        self.app.text_selection.decorate_visible_lines(
            pane_text_area(
                self.request.area,
                self.request.content_width,
                self.request.visible_height,
            ),
            lines,
            line_scroll,
            self.request.visible_height,
            self.request.theme,
        )
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
        let (title, visible_output_lines) = pane_lines(self.app, output_height);
        lines.extend(visible_output_lines);
        lines.truncate(self.request.visible_height);
        let lines = self.decorate_visible_lines(lines, 0);

        DiffPaneRows {
            title,
            lines,
            scrollbar: None,
        }
    }
}
