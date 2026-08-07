use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{CommitDetails, DiffFile};
use crate::theme::Theme;

use super::diff::diff_lines_until;
use super::file_summary::{file_header_label, file_icon, file_icon_color, file_status_suffix};
use super::text::{color_style, wrap_line};

pub(crate) fn commit_details_lines(
    details: &CommitDetails,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_field(&mut lines, "Commit", &details.id, content_width, theme);
    push_field(&mut lines, "Author", &details.author, content_width, theme);
    push_field(
        &mut lines,
        "Date",
        &details.committed_at,
        content_width,
        theme,
    );
    lines.push(Line::raw(""));
    push_section(&mut lines, "Message", theme);
    if details.message.is_empty() {
        lines.push(muted_line("(empty commit message)", theme));
    } else {
        for message_line in details.message.split('\n') {
            lines.extend(wrap_line(
                Line::styled(
                    message_line.to_string(),
                    color_style(theme.text, theme.background),
                ),
                content_width,
            ));
        }
    }

    lines.push(Line::raw(""));
    push_section(&mut lines, &file_summary(details), theme);
    if details.changeset.files.is_empty() {
        lines.push(muted_line("No files changed.", theme));
    } else {
        for file in &details.changeset.files {
            lines.extend(file_summary_lines(file, content_width, theme));
        }
    }

    lines.push(Line::raw(""));
    push_section(&mut lines, "Diff", theme);
    if details.changeset.files.is_empty() {
        lines.push(muted_line("No diff for this commit.", theme));
    } else {
        for file in &details.changeset.files {
            lines.extend(
                diff_lines_until(file, content_width, theme, false, None, usize::MAX).lines,
            );
        }
    }
    lines
}

fn push_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    content_width: usize,
    theme: Theme,
) {
    lines.extend(wrap_line(
        Line::from(vec![
            Span::styled(
                format!("{label}: "),
                color_style(theme.muted, theme.background).add_modifier(Modifier::BOLD),
            ),
            Span::styled(value.to_string(), color_style(theme.text, theme.background)),
        ]),
        content_width,
    ));
}

fn push_section(lines: &mut Vec<Line<'static>>, label: &str, theme: Theme) {
    lines.push(Line::styled(
        label.to_string(),
        color_style(theme.accent, theme.background).add_modifier(Modifier::BOLD),
    ));
}

fn file_summary(details: &CommitDetails) -> String {
    let files = details.changeset.files.len();
    let additions: usize = details
        .changeset
        .files
        .iter()
        .map(|file| file.additions)
        .sum();
    let deletions: usize = details
        .changeset
        .files
        .iter()
        .map(|file| file.deletions)
        .sum();
    let noun = if files == 1 { "file" } else { "files" };
    format!("Files: {files} {noun} changed, +{additions} -{deletions}")
}

fn file_summary_lines(file: &DiffFile, content_width: usize, theme: Theme) -> Vec<Line<'static>> {
    let mut spans = vec![
        Span::styled("  ", Style::default().bg(theme.background)),
        Span::styled(
            format!("{} ", file_icon(file.display_path())),
            color_style(
                file_icon_color(file.display_path(), theme),
                theme.background,
            ),
        ),
        Span::styled(
            format!(
                "{}{}",
                file_header_label(file),
                file_status_suffix(file.status)
            ),
            color_style(theme.text, theme.background),
        ),
    ];
    if file.additions > 0 {
        spans.push(Span::styled(
            format!("  +{}", file.additions),
            color_style(theme.added, theme.background),
        ));
    }
    if file.deletions > 0 {
        spans.push(Span::styled(
            format!(" -{}", file.deletions),
            color_style(theme.removed, theme.background),
        ));
    }
    wrap_line(Line::from(spans), content_width)
}

fn muted_line(text: &str, theme: Theme) -> Line<'static> {
    Line::styled(text.to_string(), color_style(theme.muted, theme.background))
}
