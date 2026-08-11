//! Stateful local-branch and recent-commit panels.
//!
//! This module owns repository overview data, selection, scrolling, row
//! rendering, identity-preserving reloads, copy context, and pointer geometry.
//! `App` owns focus and translates commit selection into runtime effects.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{CommitSummary, LocalBranch, RepositoryOverview};
use crate::scroll_text::VerticalDirection;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryPane {
    Branches,
    Commits,
}

impl RepositoryPane {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Branches => " Branches ",
            Self::Commits => " Commits ",
        }
    }

    pub(super) fn copy_keybind_hint(self) -> &'static str {
        match self {
            Self::Branches => "copy branch",
            Self::Commits => "copy commit",
        }
    }
}

#[derive(Debug)]
pub(super) struct RepositoryPanels {
    overview: RepositoryOverview,
    branches: RepositoryList,
    commits: RepositoryList,
}

#[derive(Debug, Default)]
struct RepositoryList {
    selected_index: usize,
    scroll: usize,
    viewport: PanelViewport,
}

#[derive(Debug, Default)]
struct PanelViewport {
    area: Option<Rect>,
    first_index: usize,
    row_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RepositoryPanelTarget {
    pane: RepositoryPane,
    index: usize,
}

impl RepositoryPanelTarget {
    pub(super) fn pane(self) -> RepositoryPane {
        self.pane
    }
}

pub(super) struct RepositoryPanelRender {
    pub(super) pane: RepositoryPane,
    pub(super) area: Rect,
    pub(super) content_width: usize,
    pub(super) visible_height: usize,
    pub(super) theme: Theme,
    pub(super) focused: bool,
}

pub(super) struct RepositoryCopy {
    text: String,
    success_message: &'static str,
}

impl RepositoryCopy {
    pub(super) fn into_parts(self) -> (String, &'static str) {
        (self.text, self.success_message)
    }
}

impl RepositoryPanels {
    pub(super) fn new(overview: RepositoryOverview) -> Self {
        Self {
            overview,
            branches: RepositoryList::default(),
            commits: RepositoryList::default(),
        }
    }

    pub(super) fn invalidate_frame(&mut self) {
        self.branches.viewport.invalidate();
        self.commits.viewport.invalidate();
    }

    pub(super) fn render(&mut self, request: RepositoryPanelRender) -> Vec<Line<'static>> {
        match request.pane {
            RepositoryPane::Branches => render_list(
                &mut self.branches,
                &self.overview.branches,
                request,
                "No local branches",
                branch_line,
            ),
            RepositoryPane::Commits => render_list(
                &mut self.commits,
                &self.overview.commits,
                request,
                "No commits",
                commit_line,
            ),
        }
    }

    pub(super) fn pane_at(&self, column: u16, row: u16) -> Option<RepositoryPane> {
        if self.branches.viewport.contains(column, row) {
            return Some(RepositoryPane::Branches);
        }
        if self.commits.viewport.contains(column, row) {
            return Some(RepositoryPane::Commits);
        }
        None
    }

    pub(super) fn target_at(&self, column: u16, row: u16) -> Option<RepositoryPanelTarget> {
        if let Some(index) = self.branches.viewport.index_at(column, row) {
            return Some(RepositoryPanelTarget {
                pane: RepositoryPane::Branches,
                index,
            });
        }
        self.commits
            .viewport
            .index_at(column, row)
            .map(|index| RepositoryPanelTarget {
                pane: RepositoryPane::Commits,
                index,
            })
    }

    pub(super) fn select(&mut self, target: RepositoryPanelTarget) {
        match target.pane {
            RepositoryPane::Branches => self
                .branches
                .select(target.index, self.overview.branches.len()),
            RepositoryPane::Commits => self
                .commits
                .select(target.index, self.overview.commits.len()),
        }
    }

    pub(super) fn move_selection(
        &mut self,
        pane: RepositoryPane,
        direction: VerticalDirection,
        amount: usize,
    ) -> bool {
        match pane {
            RepositoryPane::Branches => {
                self.branches
                    .move_by(direction, amount, self.overview.branches.len())
            }
            RepositoryPane::Commits => {
                self.commits
                    .move_by(direction, amount, self.overview.commits.len())
            }
        }
    }

    pub(super) fn selected_commit_revision(&self) -> Option<&str> {
        self.overview
            .commits
            .get(self.commits.selected_index)
            .map(|commit| commit.short_id.as_str())
    }

    pub(super) fn copy(&self, pane: RepositoryPane) -> Option<RepositoryCopy> {
        match pane {
            RepositoryPane::Branches => self
                .overview
                .branches
                .get(self.branches.selected_index)
                .map(|branch| RepositoryCopy {
                    text: branch.name.clone(),
                    success_message: "copied selected branch name",
                }),
            RepositoryPane::Commits => {
                self.overview
                    .commits
                    .get(self.commits.selected_index)
                    .map(|commit| RepositoryCopy {
                        text: commit.short_id.clone(),
                        success_message: "copied selected commit id",
                    })
            }
        }
    }

    pub(super) fn replace(&mut self, overview: RepositoryOverview) {
        let selected_branch = self
            .overview
            .branches
            .get(self.branches.selected_index)
            .map(|branch| branch.name.as_str());
        let selected_commit = self
            .overview
            .commits
            .get(self.commits.selected_index)
            .map(|commit| commit.short_id.as_str());

        self.branches.selected_index = selected_branch
            .and_then(|name| {
                overview
                    .branches
                    .iter()
                    .position(|branch| branch.name == name)
            })
            .unwrap_or_else(|| {
                overview
                    .branches
                    .iter()
                    .position(|branch| branch.current)
                    .unwrap_or(0)
            });
        self.commits.selected_index = selected_commit
            .and_then(|short_id| {
                overview
                    .commits
                    .iter()
                    .position(|commit| commit.short_id == short_id)
            })
            .unwrap_or(0);
        self.overview = overview;
        self.invalidate_frame();
    }
}

impl RepositoryList {
    fn select(&mut self, index: usize, item_count: usize) {
        if item_count > 0 {
            self.selected_index = index.min(item_count - 1);
        }
    }

    fn move_by(&mut self, direction: VerticalDirection, amount: usize, item_count: usize) -> bool {
        if item_count == 0 {
            return false;
        }
        self.selected_index = direction.shift_clamped(
            self.selected_index.min(item_count - 1),
            amount,
            item_count - 1,
        );
        true
    }
}

impl PanelViewport {
    fn invalidate(&mut self) {
        self.area = None;
        self.first_index = 0;
        self.row_count = 0;
    }

    fn begin(&mut self, area: Rect, first_index: usize, row_count: usize) {
        self.area = Some(area);
        self.first_index = first_index;
        self.row_count = row_count.min(usize::MAX - first_index);
    }

    fn contains(&self, column: u16, row: u16) -> bool {
        self.area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    fn index_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }
        let row_offset = usize::from(row.saturating_sub(area.y + 1));
        (row_offset < self.row_count).then(|| self.first_index + row_offset)
    }
}

fn render_list<Item>(
    state: &mut RepositoryList,
    items: &[Item],
    request: RepositoryPanelRender,
    empty_message: &'static str,
    render_item: impl Fn(&Item, Color, Theme) -> Line<'static>,
) -> Vec<Line<'static>> {
    let item_count = items.len();
    state.selected_index = state.selected_index.min(item_count.saturating_sub(1));
    state.scroll = visible_scroll(
        item_count,
        state.selected_index,
        state.scroll,
        request.visible_height,
    );
    let selected_index = (request.focused && item_count > 0).then_some(state.selected_index);
    let lines = repository_lines(
        items,
        selected_index,
        request.content_width,
        empty_message,
        request.theme,
        render_item,
    )
    .into_iter()
    .skip(state.scroll)
    .take(request.visible_height)
    .collect::<Vec<_>>();
    let mapped_row_count = lines.len().min(item_count.saturating_sub(state.scroll));
    state
        .viewport
        .begin(request.area, state.scroll, mapped_row_count);
    lines
}

fn visible_scroll(
    item_count: usize,
    selected_index: usize,
    scroll: usize,
    visible_height: usize,
) -> usize {
    if item_count == 0 {
        return 0;
    }

    let visible_height = visible_height.max(1);
    let selected_index = selected_index.min(item_count - 1);
    let max_scroll = item_count.saturating_sub(visible_height);
    if selected_index < scroll {
        return selected_index.min(max_scroll);
    }
    if selected_index >= scroll.saturating_add(visible_height) {
        return selected_index
            .saturating_add(1)
            .saturating_sub(visible_height)
            .min(max_scroll);
    }
    scroll.min(max_scroll)
}

fn repository_lines<Item>(
    items: &[Item],
    selected_index: Option<usize>,
    content_width: usize,
    empty_message: &'static str,
    theme: Theme,
    render: impl Fn(&Item, Color, Theme) -> Line<'static>,
) -> Vec<Line<'static>> {
    if items.is_empty() {
        return vec![Line::styled(
            empty_message,
            color_style(theme.muted, theme.background),
        )];
    }

    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let selected = selected_index == Some(index);
            let background = if selected {
                theme.selected
            } else {
                theme.background
            };
            let mut line = render(item, background, theme);
            if selected {
                let padding = content_width.saturating_sub(line.width());
                line.spans.push(Span::styled(
                    " ".repeat(padding),
                    color_style(theme.text, theme.selected),
                ));
            }
            line
        })
        .collect()
}

fn branch_line(branch: &LocalBranch, background: Color, theme: Theme) -> Line<'static> {
    let color = if branch.current {
        theme.accent
    } else {
        theme.text
    };
    let marker_style = color_style(color, background);
    let name_style = if branch.current {
        marker_style.add_modifier(Modifier::BOLD)
    } else {
        marker_style
    };

    Line::from(vec![
        Span::styled(if branch.current { "* " } else { "  " }, marker_style),
        Span::styled(branch.name.clone(), name_style),
    ])
}

fn commit_line(commit: &CommitSummary, background: Color, theme: Theme) -> Line<'static> {
    let text_style = color_style(theme.text, background);
    Line::from(vec![
        Span::styled(
            commit.short_id.clone(),
            color_style(theme.accent, background),
        ),
        Span::styled(" ", text_style),
        Span::styled(commit.subject.clone(), text_style),
    ])
}

fn color_style(foreground: Color, background: Color) -> Style {
    Style::default().fg(foreground).bg(background)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_branch_and_commit_rows_keep_repository_formatting() {
        let theme = Theme::github_dark();
        let branches = repository_lines(
            &[LocalBranch {
                name: "main".to_string(),
                current: true,
            }],
            Some(0),
            6,
            "No local branches",
            theme,
            branch_line,
        );
        let commits = repository_lines(
            &[CommitSummary {
                short_id: "abc1234".to_string(),
                subject: "Add repository panels".to_string(),
            }],
            Some(0),
            29,
            "No commits",
            theme,
            commit_line,
        );

        assert_eq!(line_text(&branches[0]), "* main");
        assert_eq!(line_text(&commits[0]), "abc1234 Add repository panels");
        assert_eq!(branches[0].spans[1].style.bg, Some(theme.selected));
    }

    #[test]
    fn empty_repository_panels_have_messages() {
        let theme = Theme::github_dark();
        let branches: Vec<LocalBranch> = Vec::new();
        let commits: Vec<CommitSummary> = Vec::new();

        assert_eq!(
            line_text(
                &repository_lines(&branches, None, 20, "No local branches", theme, branch_line,)[0]
            ),
            "No local branches"
        );
        assert_eq!(
            line_text(&repository_lines(&commits, None, 20, "No commits", theme, commit_line,)[0]),
            "No commits"
        );
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
