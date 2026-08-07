use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::model::CommitDetails;
use crate::rows;
use crate::scroll_text::{ScrollText, VerticalDirection};
use crate::theme::Theme;

#[derive(Debug)]
pub(super) struct CommitDetailsPane {
    active: bool,
    state: CommitDetailsState,
    scroll: ScrollText,
    area: Option<Rect>,
    render_cache: Option<CommitDetailsRenderCache>,
}

#[derive(Debug)]
enum CommitDetailsState {
    Empty,
    Loading {
        revision: String,
    },
    Loaded {
        revision: String,
        details: CommitDetails,
    },
    Failed {
        revision: String,
        message: String,
    },
}

impl CommitDetailsState {
    fn revision(&self) -> Option<&str> {
        match self {
            Self::Loading { revision }
            | Self::Loaded { revision, .. }
            | Self::Failed { revision, .. } => Some(revision),
            Self::Empty => None,
        }
    }

    fn is_loading_or_loaded(&self, revision: &str) -> bool {
        matches!(self, Self::Loading { .. } | Self::Loaded { .. })
            && self.revision() == Some(revision)
    }
}

#[derive(Debug)]
struct CommitDetailsRenderCache {
    revision: String,
    content_width: usize,
    theme: Theme,
    lines: Vec<Line<'static>>,
}

pub(super) struct RenderedCommitDetails {
    pub(super) title: String,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) line_scroll: usize,
}

impl CommitDetailsPane {
    pub(super) fn new() -> Self {
        Self {
            active: false,
            state: CommitDetailsState::Empty,
            scroll: ScrollText::default(),
            area: None,
            render_cache: None,
        }
    }

    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    /// Activates `revision` and reports whether a background load is needed.
    pub(super) fn activate(&mut self, revision: &str) -> bool {
        self.active = true;
        if self.state.is_loading_or_loaded(revision) {
            return false;
        }

        self.state = CommitDetailsState::Loading {
            revision: revision.to_string(),
        };
        self.reset_content();
        true
    }

    pub(super) fn deactivate(&mut self) {
        self.active = false;
        self.area = None;
        if matches!(self.state, CommitDetailsState::Loading { .. }) {
            self.state = CommitDetailsState::Empty;
        }
    }

    pub(super) fn activate_empty(&mut self) {
        self.active = true;
        self.state = CommitDetailsState::Empty;
        self.reset_content();
    }

    pub(super) fn revision(&self) -> Option<&str> {
        self.state.revision()
    }

    pub(super) fn loaded(&mut self, revision: &str, details: CommitDetails) {
        if !self.matches_active_revision(revision) {
            return;
        }
        self.state = CommitDetailsState::Loaded {
            revision: revision.to_string(),
            details,
        };
        self.reset_content();
    }

    pub(super) fn failed(&mut self, revision: &str, message: String) {
        if !self.matches_active_revision(revision) {
            return;
        }
        self.state = CommitDetailsState::Failed {
            revision: revision.to_string(),
            message,
        };
        self.reset_content();
    }

    fn matches_active_revision(&self, revision: &str) -> bool {
        self.active && self.revision().is_some_and(|current| current == revision)
    }

    fn reset_content(&mut self) {
        self.scroll = ScrollText::default();
        self.render_cache = None;
        self.area = None;
    }

    pub(super) fn invalidate_frame(&mut self) {
        self.area = None;
    }

    pub(super) fn render(
        &mut self,
        area: Rect,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> RenderedCommitDetails {
        self.area = Some(area);
        let title = self.title();
        let (lines, line_scroll) = self.visible_lines(content_width, visible_height, theme);
        RenderedCommitDetails {
            title,
            lines,
            line_scroll,
        }
    }

    fn title(&self) -> String {
        let revision = self.revision().unwrap_or("commit");
        format!(" Commit {} ", &revision[..revision.len().min(12)])
    }

    fn visible_lines(
        &mut self,
        content_width: usize,
        visible_height: usize,
        theme: Theme,
    ) -> (Vec<Line<'static>>, usize) {
        match &self.state {
            CommitDetailsState::Empty => {
                let lines = rows::no_diff_lines("No commit selected.", content_width, theme);
                Self::visible_lines_from(&mut self.scroll, &lines, visible_height)
            }
            CommitDetailsState::Loading { .. } => {
                let lines = rows::no_diff_lines("Loading commit details...", content_width, theme);
                Self::visible_lines_from(&mut self.scroll, &lines, visible_height)
            }
            CommitDetailsState::Failed { message, .. } => {
                let message = format!("Git error: {message}");
                let lines = rows::live_status_lines(Some(&message), None, content_width, theme);
                Self::visible_lines_from(&mut self.scroll, &lines, visible_height)
            }
            CommitDetailsState::Loaded { details, .. } => {
                if !self.render_cache.as_ref().is_some_and(|cache| {
                    cache.revision == details.id
                        && cache.content_width == content_width
                        && cache.theme == theme
                }) {
                    self.render_cache = Some(CommitDetailsRenderCache {
                        revision: details.id.clone(),
                        content_width,
                        theme,
                        lines: rows::commit_details_lines(details, content_width, theme),
                    });
                }
                let lines = &self
                    .render_cache
                    .as_ref()
                    .expect("loaded commit details must have a render cache")
                    .lines;
                Self::visible_lines_from(&mut self.scroll, lines, visible_height)
            }
        }
    }

    fn visible_lines_from(
        scroll: &mut ScrollText,
        lines: &[Line<'static>],
        visible_height: usize,
    ) -> (Vec<Line<'static>>, usize) {
        scroll.sync(lines.len(), visible_height);
        let line_scroll = scroll.offset();
        let visible_lines = lines
            .iter()
            .skip(line_scroll)
            .take(visible_height)
            .cloned()
            .collect();
        (visible_lines, line_scroll)
    }

    pub(super) fn contains(&self, column: u16, row: u16) -> bool {
        self.area.is_some_and(|area| {
            column >= area.x
                && column < area.x.saturating_add(area.width)
                && row >= area.y
                && row < area.y.saturating_add(area.height)
        })
    }

    pub(super) fn scroll_page(&mut self, direction: VerticalDirection) {
        self.scroll.scroll_by(direction, self.scroll.page());
    }

    pub(super) fn scroll_by(&mut self, direction: VerticalDirection, amount: usize) {
        self.scroll.scroll_by(direction, amount);
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.scroll.scroll_to_top();
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll.scroll_to_bottom();
    }
}
