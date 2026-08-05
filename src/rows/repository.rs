use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};

use crate::model::{CommitSummary, LocalBranch};
use crate::theme::Theme;

use super::text::{color_style, muted_line};

pub(crate) fn branch_lines(
    branches: &[LocalBranch],
    selected_index: Option<usize>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    repository_lines(
        branches,
        selected_index,
        content_width,
        "No local branches",
        theme,
        |branch, background| {
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
        },
    )
}

pub(crate) fn commit_lines(
    commits: &[CommitSummary],
    selected_index: Option<usize>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    repository_lines(
        commits,
        selected_index,
        content_width,
        "No commits",
        theme,
        |commit, background| {
            let text_style = color_style(theme.text, background);
            Line::from(vec![
                Span::styled(
                    commit.short_id.clone(),
                    color_style(theme.accent, background),
                ),
                Span::styled(" ", text_style),
                Span::styled(commit.subject.clone(), text_style),
            ])
        },
    )
}

fn repository_lines<Item>(
    items: &[Item],
    selected_index: Option<usize>,
    content_width: usize,
    empty_message: &'static str,
    theme: Theme,
    render: impl Fn(&Item, Color) -> Line<'static>,
) -> Vec<Line<'static>> {
    if items.is_empty() {
        return vec![muted_line(empty_message, theme)];
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
            let mut line = render(item, background);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_branch_is_marked_and_empty_panels_have_messages() {
        let theme = Theme::github_dark();
        let branches = branch_lines(
            &[LocalBranch {
                name: "main".to_string(),
                current: true,
            }],
            Some(0),
            6,
            theme,
        );

        assert_eq!(line_text(&branches[0]), "* main");
        assert_eq!(
            line_text(&branch_lines(&[], None, 20, theme)[0]),
            "No local branches"
        );
        assert_eq!(
            line_text(&commit_lines(&[], None, 20, theme)[0]),
            "No commits"
        );
    }

    #[test]
    fn commits_include_short_id_and_subject() {
        let lines = commit_lines(
            &[CommitSummary {
                short_id: "abc1234".to_string(),
                subject: "Add repository panels".to_string(),
            }],
            Some(0),
            29,
            Theme::github_dark(),
        );

        assert_eq!(line_text(&lines[0]), "abc1234 Add repository panels");
    }

    #[test]
    fn selected_repository_rows_use_selected_background() {
        let theme = Theme::github_dark();
        let branches = branch_lines(
            &[
                LocalBranch {
                    name: "main".to_string(),
                    current: true,
                },
                LocalBranch {
                    name: "topic".to_string(),
                    current: false,
                },
            ],
            Some(1),
            12,
            theme,
        );

        assert_eq!(branches[0].spans[1].style.bg, Some(theme.background));
        assert_eq!(branches[1].spans[1].style.bg, Some(theme.selected));
        assert_eq!(branches[1].width(), 12);
        assert_eq!(
            branches[1].spans.last().unwrap().style.bg,
            Some(theme.selected)
        );
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
