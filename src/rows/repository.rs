use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::model::{CommitSummary, LocalBranch};
use crate::theme::Theme;

use super::text::{color_style, muted_line};

pub(crate) fn branch_lines(branches: &[LocalBranch], theme: Theme) -> Vec<Line<'static>> {
    if branches.is_empty() {
        return vec![muted_line("No local branches", theme)];
    }

    branches
        .iter()
        .map(|branch| {
            let color = if branch.current {
                theme.accent
            } else {
                theme.text
            };
            let mut name_style = color_style(color, theme.background);
            if branch.current {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }

            Line::from(vec![
                Span::styled(
                    if branch.current { "* " } else { "  " },
                    color_style(color, theme.background),
                ),
                Span::styled(branch.name.clone(), name_style),
            ])
        })
        .collect()
}

pub(crate) fn commit_lines(commits: &[CommitSummary], theme: Theme) -> Vec<Line<'static>> {
    if commits.is_empty() {
        return vec![muted_line("No commits", theme)];
    }

    commits
        .iter()
        .map(|commit| {
            Line::from(vec![
                Span::styled(
                    commit.short_id.clone(),
                    color_style(theme.accent, theme.background),
                ),
                Span::styled(" ", color_style(theme.text, theme.background)),
                Span::styled(
                    commit.subject.clone(),
                    color_style(theme.text, theme.background),
                ),
            ])
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
            theme,
        );

        assert_eq!(line_text(&branches[0]), "* main");
        assert_eq!(line_text(&branch_lines(&[], theme)[0]), "No local branches");
        assert_eq!(line_text(&commit_lines(&[], theme)[0]), "No commits");
    }

    #[test]
    fn commits_include_short_id_and_subject() {
        let lines = commit_lines(
            &[CommitSummary {
                short_id: "abc1234".to_string(),
                subject: "Add repository panels".to_string(),
            }],
            Theme::github_dark(),
        );

        assert_eq!(line_text(&lines[0]), "abc1234 Add repository panels");
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
