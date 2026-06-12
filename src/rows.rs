//! Rendered terminal rows for sidebar, diff content, and status bars.
//!
//! This module is the row-rendering interface. Submodules own focused pieces of
//! the implementation: sidebar rows, diff rows, intraline emphasis, and wrapping.
//! `ui` owns pane layout and Ratatui widget drawing.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::Changeset;
use crate::theme::Theme;

mod diff;
mod file_summary;
mod intraline;
mod sidebar;
mod text;

pub(crate) use diff::{diff_layout_counts, diff_lines_until, selected_hunk_header_rows};
pub(crate) use sidebar::SidebarRowsInput;
pub(crate) use sidebar::{sidebar_row_counts, sidebar_rows};

use text::{color_style, muted_line, wrap_line};

pub(crate) const DIFF_PREFETCH_ROWS: usize = 120;
const RAIL_MARKER: &str = "▌";

pub(crate) fn no_diff_lines(
    message: &'static str,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    wrap_line(muted_line(message, theme), content_width)
}

pub(crate) fn live_status_lines(
    error: Option<&str>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(error) = error else {
        return Vec::new();
    };

    wrap_line(
        Line::styled(
            format!("! {error}"),
            color_style(theme.removed, theme.background),
        ),
        content_width,
    )
}

pub(crate) fn discard_status_lines(
    prompt: Option<&str>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(prompt) = prompt else {
        return Vec::new();
    };

    wrap_line(
        Line::styled(
            format!("! {prompt}  y/Enter confirm  Esc/n cancel"),
            color_style(theme.removed, theme.background),
        ),
        content_width,
    )
}

pub(crate) enum SearchStatus<'a> {
    Prompt {
        input: &'a str,
    },
    Active {
        query: &'a str,
        active: Option<usize>,
        total: usize,
    },
}

pub(crate) fn search_status_lines(
    status: Option<SearchStatus<'_>>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(status) = status else {
        return Vec::new();
    };

    let text = match status {
        SearchStatus::Prompt { input: "" } => {
            "/ type search, Enter to apply, Esc to cancel".to_string()
        }
        SearchStatus::Prompt { input } => format!("/ {input}"),
        SearchStatus::Active {
            query, total: 0, ..
        } => format!("Search: {query:?}  no matches"),
        SearchStatus::Active {
            query,
            active,
            total,
        } => format!(
            "Search: {query:?}  {}/{}  [n/N] next/prev  [Esc] clear",
            active.unwrap_or(0),
            total
        ),
    };

    wrap_line(
        Line::styled(text, color_style(theme.accent, theme.background)),
        content_width,
    )
}

pub(crate) fn keybind_bar_line(
    files_panel_visible: bool,
    can_stage: bool,
    stage_hint: Option<&'static str>,
    discard_hint: Option<&'static str>,
    theme: Theme,
) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.accent, background).add_modifier(Modifier::BOLD);
    let label_style = color_style(theme.muted, background);
    let separator_style = color_style(theme.border, background);

    let mut hints: Vec<(&'static str, &'static str)> = vec![(
        "f",
        if files_panel_visible {
            "hide files"
        } else {
            "show files"
        },
    )];
    if files_panel_visible {
        hints.push(("Tab", "focus"));
    }
    if let Some(stage_hint) = stage_hint {
        hints.push(("Space", stage_hint));
    }
    if let Some(discard_hint) = discard_hint {
        hints.push(("d", discard_hint));
    }
    hints.push(("/", "search"));
    hints.push(("j/k", "move"));
    hints.push(("?", "help"));
    hints.push(("q", "quit"));

    let mode_label = if can_stage { " DIFF " } else { " REVIEW " };
    let mut spans = vec![
        Span::styled(
            mode_label,
            color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", color_style(theme.accent, background)),
        Span::styled("  ", label_style),
    ];

    for (index, (key, label)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  \u{b7}  ", separator_style));
        }
        spans.push(Span::styled(*key, key_style));
        spans.push(Span::styled(format!(" {label}"), label_style));
    }

    Line::from(spans)
}

pub(crate) fn help_overlay_lines(
    can_stage: bool,
    can_discard: bool,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    push_help_section(&mut lines, "Global", theme);
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("?"),
            HelpSegment::text(" help/dismiss   "),
            HelpSegment::command("q"),
            HelpSegment::text(" close help or quit   "),
            HelpSegment::command("Ctrl-c"),
            HelpSegment::text(" quit"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("f"),
            HelpSegment::text(" files   "),
            HelpSegment::command("Tab"),
            HelpSegment::text("/"),
            HelpSegment::command("Left"),
            HelpSegment::text("/"),
            HelpSegment::command("Right"),
            HelpSegment::text("/"),
            HelpSegment::command("Enter"),
            HelpSegment::text(" focus panes"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("/"),
            HelpSegment::text(" search, "),
            HelpSegment::command("Enter"),
            HelpSegment::text(" apply, "),
            HelpSegment::command("Esc"),
            HelpSegment::text(" cancel or clear"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("n"),
            HelpSegment::text("/"),
            HelpSegment::command("N"),
            HelpSegment::text(" next or previous match/hunk   "),
            HelpSegment::command("g"),
            HelpSegment::text("/"),
            HelpSegment::command("Home"),
            HelpSegment::text(" top   "),
            HelpSegment::command("G"),
            HelpSegment::text("/"),
            HelpSegment::command("End"),
            HelpSegment::text(" bottom"),
        ],
        content_width,
        theme,
    );

    push_help_section(&mut lines, "Sidebar", theme);
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("j"),
            HelpSegment::text("/"),
            HelpSegment::command("k"),
            HelpSegment::text(" select file"),
        ],
        content_width,
        theme,
    );

    push_help_section(&mut lines, "Diff", theme);
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("j"),
            HelpSegment::text("/"),
            HelpSegment::command("k"),
            HelpSegment::text(" scroll row   "),
            HelpSegment::command("PageDown"),
            HelpSegment::text("/"),
            HelpSegment::command("PageUp"),
            HelpSegment::text(" page"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("Ctrl-d"),
            HelpSegment::text("/"),
            HelpSegment::command("Ctrl-u"),
            HelpSegment::text(" page   "),
            HelpSegment::command("n"),
            HelpSegment::text("/"),
            HelpSegment::command("N"),
            HelpSegment::text(" next or previous hunk"),
        ],
        content_width,
        theme,
    );

    push_help_section(&mut lines, "Mouse", theme);
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("hover"),
            HelpSegment::text(" focus pane   "),
            HelpSegment::command("click file"),
            HelpSegment::text(" select"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("click hunk"),
            HelpSegment::text(" select   "),
            HelpSegment::command("wheel"),
            HelpSegment::text(" scroll pointed pane"),
        ],
        content_width,
        theme,
    );

    push_help_section(&mut lines, "Worktree-only", theme);
    if can_stage || can_discard {
        if can_stage {
            push_help_line(
                &mut lines,
                &[
                    HelpSegment::command("Space"),
                    HelpSegment::text(" stage/unstage focused file or hunk"),
                ],
                content_width,
                theme,
            );
        }
        if can_discard {
            push_help_line(
                &mut lines,
                &[
                    HelpSegment::command("d"),
                    HelpSegment::text(" discard focused file or hunk, "),
                    HelpSegment::command("y"),
                    HelpSegment::text("/"),
                    HelpSegment::command("Enter"),
                    HelpSegment::text(" confirm"),
                ],
                content_width,
                theme,
            );
        }
        push_help_line(
            &mut lines,
            &[
                HelpSegment::command("e"),
                HelpSegment::text(" open selected file in $EDITOR"),
            ],
            content_width,
            theme,
        );
    } else {
        push_help_line(
            &mut lines,
            &[HelpSegment::muted(
                "Worktree actions unavailable in PR mode",
            )],
            content_width,
            theme,
        );
    }

    lines
}

fn push_help_section(lines: &mut Vec<Line<'static>>, title: &'static str, theme: Theme) {
    if !lines.is_empty() {
        lines.push(Line::styled("", help_style(theme.text, theme)));
    }

    lines.push(Line::styled(
        title,
        help_style(theme.accent, theme).add_modifier(Modifier::BOLD),
    ));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpSegment {
    Command(&'static str),
    Text(&'static str),
    Muted(&'static str),
}

impl HelpSegment {
    const fn command(text: &'static str) -> Self {
        Self::Command(text)
    }

    const fn text(text: &'static str) -> Self {
        Self::Text(text)
    }

    const fn muted(text: &'static str) -> Self {
        Self::Muted(text)
    }
}

fn push_help_line(
    lines: &mut Vec<Line<'static>>,
    segments: &[HelpSegment],
    content_width: usize,
    theme: Theme,
) {
    lines.extend(wrap_line(help_line(segments, theme), content_width));
}

fn help_line(segments: &[HelpSegment], theme: Theme) -> Line<'static> {
    let spans: Vec<Span<'static>> = segments
        .iter()
        .map(|segment| match *segment {
            HelpSegment::Command(text) => Span::styled(text, help_command_style(theme)),
            HelpSegment::Text(text) => Span::styled(text, help_style(theme.text, theme)),
            HelpSegment::Muted(text) => Span::styled(text, help_style(theme.muted, theme)),
        })
        .collect();

    Line::from(spans)
}

fn help_command_style(theme: Theme) -> Style {
    help_style(theme.accent, theme).add_modifier(Modifier::BOLD)
}

fn help_style(foreground: Color, theme: Theme) -> Style {
    color_style(foreground, theme.background_alt)
}

pub(crate) fn changeset_title(changeset: &Changeset) -> String {
    let additions: usize = changeset.files.iter().map(|file| file.additions).sum();
    let deletions: usize = changeset.files.iter().map(|file| file.deletions).sum();
    let title = if changeset.title.is_empty() {
        "changeset"
    } else {
        &changeset.title
    };

    format!("{}  +{}  -{}", title, additions, deletions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_overlay_lists_keymap_sections() {
        let help = help_text(true);

        for section in ["Global", "Sidebar", "Diff", "Mouse", "Worktree-only"] {
            assert!(help.contains(section), "missing {section} section: {help}");
        }
    }

    #[test]
    fn help_overlay_reflects_staging_availability() {
        let worktree_help = help_text(true);
        let pr_help = help_text(false);

        assert!(worktree_help.contains("Space stage/unstage focused file or hunk"));
        assert!(worktree_help.contains("d discard focused file or hunk"));
        assert!(worktree_help.contains("e open selected file in $EDITOR"));
        assert!(pr_help.contains("Worktree actions unavailable in PR mode"));
        assert!(!pr_help.contains("Space stage/unstage focused file or hunk"));
        assert!(!pr_help.contains("d discard focused file or hunk"));
    }

    #[test]
    fn help_overlay_styles_command_tokens_for_contrast() {
        let theme = Theme::github_dark();
        let lines = help_overlay_lines(true, true, 80, theme);

        let command_span = find_span(&lines, "?").expect("command span should render");
        assert_eq!(command_span.style.fg, Some(theme.accent));
        assert!(command_span.style.add_modifier.contains(Modifier::BOLD));

        let description_span =
            find_span(&lines, " help/dismiss   ").expect("description span should render");
        assert_eq!(description_span.style.fg, Some(theme.text));
        assert!(!description_span.style.add_modifier.contains(Modifier::BOLD));
    }

    fn help_text(can_stage: bool) -> String {
        help_overlay_lines(can_stage, can_stage, 80, Theme::github_dark())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find_span<'a>(lines: &'a [Line<'_>], text: &str) -> Option<&'a Span<'a>> {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == text)
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
