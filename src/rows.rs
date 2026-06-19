//! Rendered terminal rows for sidebar, diff content, and status bars.
//!
//! This module is the row-rendering interface. Submodules own focused pieces of
//! the implementation: sidebar rows, diff rows, intraline emphasis, and wrapping.
//! `ui` owns pane layout and Ratatui widget drawing.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::ask_ai::AskAiResult;
use crate::custom_command::{CustomCommandBinding, CustomCommandResult};
use crate::model::Changeset;
use crate::theme::Theme;

mod diff;
mod file_summary;
mod intraline;
mod markdown;
mod sidebar;
mod text;

pub(crate) use diff::{diff_layout_counts, diff_lines_until, selected_hunk_header_rows};
pub(crate) use sidebar::SidebarRowsInput;
pub(crate) use sidebar::{sidebar_row_counts, sidebar_rows};

use text::{color_style, muted_line, wrap_line};

pub(crate) const DIFF_PREFETCH_ROWS: usize = 120;
const CUSTOM_COMMAND_SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
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
    notice: Option<&str>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let (prefix, message, color) = if let Some(error) = error {
        ("!", error, theme.removed)
    } else if let Some(notice) = notice {
        ("ok", notice, theme.accent)
    } else {
        return Vec::new();
    };

    wrap_line(
        Line::styled(
            format!("{prefix} {message}"),
            color_style(color, theme.background),
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

pub(crate) fn custom_command_running_lines(
    command: Option<&CustomCommandBinding>,
    spinner_frame: usize,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(command) = command else {
        return Vec::new();
    };

    wrap_line(
        Line::styled(
            format!(
                "{} Running command: {}",
                CUSTOM_COMMAND_SPINNER_FRAMES[spinner_frame % CUSTOM_COMMAND_SPINNER_FRAMES.len()],
                command.label()
            ),
            color_style(theme.accent, theme.background),
        ),
        content_width,
    )
}

pub(crate) fn ask_ai_prompt_lines(
    input: Option<&str>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(input) = input else {
        return Vec::new();
    };

    let text = if input.is_empty() {
        "Ask AI: type a question, Enter submit, Esc cancel".to_string()
    } else {
        format!("Ask AI: {input}")
    };

    wrap_line(
        Line::styled(text, color_style(theme.accent, theme.background)),
        content_width,
    )
}

pub(crate) fn ask_ai_running_lines(
    question: Option<&str>,
    spinner_frame: usize,
    cancelling: bool,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(question) = question else {
        return Vec::new();
    };

    let status = if cancelling {
        "Cancelling Ask AI"
    } else {
        "Asking AI"
    };
    wrap_line(
        Line::styled(
            format!(
                "{} {status}: {question}",
                CUSTOM_COMMAND_SPINNER_FRAMES[spinner_frame % CUSTOM_COMMAND_SPINNER_FRAMES.len()]
            ),
            color_style(theme.accent, theme.background),
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
    stage_hint: Option<&'static str>,
    theme: Theme,
) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD);
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
    hints.push(("/", "search"));
    hints.push(("j/k", "move"));
    hints.push(("?", "help"));
    hints.push(("q", "quit"));

    let mut spans = Vec::new();

    for (index, (key, label)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  \u{b7}  ", separator_style));
        }
        spans.push(keybind_key_span(key, key_style));
        spans.push(Span::styled(format!(" {label}"), label_style));
    }

    Line::from(spans)
}

pub(crate) fn keybind_mode_tag_line(can_stage: bool, theme: Theme) -> Line<'static> {
    let background = theme.background;
    let mode_label = if can_stage { " DIFF " } else { " REVIEW " };
    Line::from(vec![
        Span::styled(
            mode_label,
            color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", color_style(theme.accent, background)),
    ])
}

fn keybind_key_span(key: &'static str, style: Style) -> Span<'static> {
    Span::styled(format!(" {key} "), style)
}

pub(crate) fn ask_ai_prompt_keybind_bar_line(theme: Theme) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD);
    let label_style = color_style(theme.muted, background);
    let separator_style = color_style(theme.border, background);

    Line::from(vec![
        Span::styled(
            " ASK AI ",
            color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", color_style(theme.accent, background)),
        Span::styled("  ", label_style),
        keybind_key_span("Enter", key_style),
        Span::styled(" submit", label_style),
        Span::styled("  \u{b7}  ", separator_style),
        keybind_key_span("Esc", key_style),
        Span::styled(" cancel", label_style),
    ])
}

pub(crate) fn ask_ai_running_keybind_bar_line(theme: Theme) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD);
    let label_style = color_style(theme.muted, background);
    let separator_style = color_style(theme.border, background);

    Line::from(vec![
        Span::styled(
            " ASK AI ",
            color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", color_style(theme.accent, background)),
        Span::styled("  ", label_style),
        keybind_key_span("Esc/q", key_style),
        Span::styled(" cancel", label_style),
        Span::styled("  \u{b7}  ", separator_style),
        keybind_key_span("Ctrl-c", key_style),
        Span::styled(" quit", label_style),
    ])
}

pub(crate) fn ask_ai_output_keybind_bar_line(theme: Theme) -> Line<'static> {
    output_keybind_bar_line(" ASK AI ", true, theme)
}

pub(crate) fn custom_command_output_keybind_bar_line(theme: Theme) -> Line<'static> {
    output_keybind_bar_line(" COMMAND ", false, theme)
}

fn output_keybind_bar_line(
    tag: &'static str,
    include_copy_hint: bool,
    theme: Theme,
) -> Line<'static> {
    let mut hints: Vec<(&'static str, &'static str)> = vec![
        ("j/k", "scroll"),
        ("Ctrl-d/Ctrl-u", "page"),
        ("g/G", "top/bottom"),
    ];
    if include_copy_hint {
        hints.push(("y", "copy"));
    }
    hints.push(("Esc/q", "close"));

    tagged_keybind_bar_line(tag, &hints, theme)
}

fn tagged_keybind_bar_line(
    tag: &'static str,
    hints: &[(&'static str, &'static str)],
    theme: Theme,
) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD);
    let label_style = color_style(theme.muted, background);
    let separator_style = color_style(theme.border, background);

    let mut spans = vec![
        Span::styled(
            tag,
            color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", color_style(theme.accent, background)),
        Span::styled("  ", label_style),
    ];

    for (index, (key, label)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  \u{b7}  ", separator_style));
        }
        spans.push(keybind_key_span(key, key_style));
        spans.push(Span::styled(format!(" {label}"), label_style));
    }

    Line::from(spans)
}

pub(crate) fn help_overlay_lines(
    can_stage: bool,
    can_discard: bool,
    custom_commands: &[CustomCommandBinding],
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

    push_custom_commands_section(&mut lines, custom_commands, content_width, theme);

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
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("r"),
            HelpSegment::text(" toggle reviewed for selected file"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("y"),
            HelpSegment::text(" copy selected file path"),
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
            HelpSegment::command("Ctrl-d"),
            HelpSegment::text("/"),
            HelpSegment::command("Ctrl-u"),
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
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("a"),
            HelpSegment::text(" Ask AI about focused file or hunk, "),
            HelpSegment::command("Enter"),
            HelpSegment::text(" submit, "),
            HelpSegment::command("Esc"),
            HelpSegment::text(" cancel"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("x"),
            HelpSegment::text(" Explain focused file or hunk with Ask AI"),
        ],
        content_width,
        theme,
    );
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("y"),
            HelpSegment::text(" copy selected hunk diff   "),
            HelpSegment::command("Y"),
            HelpSegment::text(" copy selected file diff"),
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
    push_help_line(
        &mut lines,
        &[
            HelpSegment::command("drag text"),
            HelpSegment::text(" copy selection"),
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

fn push_custom_commands_section(
    lines: &mut Vec<Line<'static>>,
    custom_commands: &[CustomCommandBinding],
    content_width: usize,
    theme: Theme,
) {
    push_help_section(lines, "Custom commands", theme);
    if custom_commands.is_empty() {
        push_help_line(
            lines,
            &[HelpSegment::muted("No custom commands configured")],
            content_width,
            theme,
        );
    } else {
        for command in custom_commands {
            push_custom_command_help_line(lines, command, content_width, theme);
        }
    }
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

fn push_custom_command_help_line(
    lines: &mut Vec<Line<'static>>,
    command: &CustomCommandBinding,
    content_width: usize,
    theme: Theme,
) {
    lines.extend(wrap_line(
        Line::from(vec![
            Span::styled(command.key_display(), help_command_style(theme)),
            Span::styled(
                format!(" {}", command.label()),
                help_style(theme.text, theme),
            ),
            Span::styled(
                format!("  {}", command.command()),
                help_style(theme.muted, theme),
            ),
        ]),
        content_width,
    ));
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

pub(crate) fn custom_command_output_lines(
    result: &CustomCommandResult,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_output_status_line(
        &mut lines,
        command_status_label(result),
        result.status_text(),
        output_status_color(result.success(), theme),
        content_width,
        theme,
    );

    if let Some(cwd) = result.cwd() {
        push_output_metadata_line(
            &mut lines,
            "cwd",
            cwd.display().to_string(),
            theme.muted,
            content_width,
            theme,
        );
    }
    push_output_text_line(
        &mut lines,
        format!("$ {}", result.command()),
        color_style(theme.accent, theme.background),
        content_width,
    );
    lines.push(Line::raw(""));

    push_output_section(
        &mut lines,
        "stdout",
        result.stdout(),
        content_width,
        color_style(theme.text, theme.background),
        theme,
    );
    lines.push(Line::raw(""));
    push_output_section(
        &mut lines,
        "stderr",
        result.stderr(),
        content_width,
        color_style(theme.removed, theme.background),
        theme,
    );

    lines
}

pub(crate) fn ask_ai_output_lines(
    result: &AskAiResult,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_output_status_line(
        &mut lines,
        ask_ai_status_label(result),
        result.status_text(),
        output_status_color(result.success(), theme),
        content_width,
        theme,
    );
    if let Some(root) = result.repo_root() {
        push_output_metadata_line(
            &mut lines,
            "repo",
            root.display().to_string(),
            theme.muted,
            content_width,
            theme,
        );
    }
    push_output_metadata_line(
        &mut lines,
        "context",
        result.context_summary(),
        theme.muted,
        content_width,
        theme,
    );
    push_output_metadata_line(
        &mut lines,
        "question",
        result.question().to_string(),
        theme.accent,
        content_width,
        theme,
    );
    lines.push(Line::raw(""));

    push_markdown_output_section(&mut lines, "answer", result.stdout(), content_width, theme);
    if !result.stderr().is_empty() {
        lines.push(Line::raw(""));
        push_output_section(
            &mut lines,
            "diagnostics",
            result.stderr(),
            content_width,
            color_style(theme.removed, theme.background),
            theme,
        );
    }

    lines
}

fn push_markdown_output_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    output: &str,
    content_width: usize,
    theme: Theme,
) {
    push_output_section_heading(lines, title, content_width, theme);
    lines.extend(markdown::markdown_lines(output, content_width, theme));
}

fn push_output_section(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    output: &str,
    content_width: usize,
    output_style: Style,
    theme: Theme,
) {
    push_output_section_heading(lines, title, content_width, theme);

    if output.is_empty() {
        push_output_text_line(
            lines,
            "(empty)".to_string(),
            color_style(theme.muted, theme.background),
            content_width,
        );
        return;
    }

    for row in output.lines() {
        push_output_text_line(lines, row.to_string(), output_style, content_width);
    }
}

fn command_status_label(result: &CustomCommandResult) -> &'static str {
    if result.success() { "OK" } else { "FAIL" }
}

fn ask_ai_status_label(result: &AskAiResult) -> &'static str {
    if result.cancelled_status() {
        "CANCELLED"
    } else if result.success() {
        "OK"
    } else {
        "FAIL"
    }
}

fn output_status_color(success: bool, theme: Theme) -> Color {
    if success { theme.added } else { theme.removed }
}

fn push_output_status_line(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    status_text: String,
    status_color: Color,
    content_width: usize,
    theme: Theme,
) {
    push_output_text_line(
        lines,
        format!("{label}  {status_text}"),
        color_style(status_color, theme.background),
        content_width,
    );
}

fn push_output_metadata_line(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    value: String,
    color: Color,
    content_width: usize,
    theme: Theme,
) {
    push_output_text_line(
        lines,
        format!("{label}: {value}"),
        color_style(color, theme.background),
        content_width,
    );
}

fn push_output_section_heading(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    content_width: usize,
    theme: Theme,
) {
    push_output_text_line(
        lines,
        title.to_string(),
        color_style(theme.accent, theme.background).add_modifier(Modifier::BOLD),
        content_width,
    );
}

fn push_output_text_line(
    lines: &mut Vec<Line<'static>>,
    text: String,
    style: Style,
    content_width: usize,
) {
    lines.extend(wrap_line(Line::styled(text, style), content_width));
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

        for section in [
            "Global",
            "Sidebar",
            "Diff",
            "Mouse",
            "Worktree-only",
            "Custom commands",
        ] {
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
        assert!(worktree_help.contains("a Ask AI about focused file or hunk"));
        assert!(worktree_help.contains("x Explain focused file or hunk with Ask AI"));
        assert!(pr_help.contains("Worktree actions unavailable in PR mode"));
        assert!(!pr_help.contains("Space stage/unstage focused file or hunk"));
        assert!(!pr_help.contains("d discard focused file or hunk"));
    }

    #[test]
    fn help_overlay_lists_review_toggle_in_sidebar_section() {
        let worktree_help = help_text(true);
        let pr_help = help_text(false);

        assert!(worktree_help.contains("r toggle reviewed for selected file"));
        assert!(pr_help.contains("r toggle reviewed for selected file"));
    }

    #[test]
    fn keybind_bar_colors_key_tokens_with_accent_fill() {
        let theme = Theme::github_dark();
        let line = keybind_bar_line(true, Some("stage file"), theme);
        let key_span = line
            .spans
            .iter()
            .find(|span| span.content.trim() == "f")
            .expect("footer should include f key");

        assert_eq!(key_span.style.fg, Some(theme.on_accent));
        assert_eq!(key_span.style.bg, Some(theme.accent));
        assert!(key_span.style.add_modifier.contains(Modifier::BOLD));
    }

    fn help_text(can_stage: bool) -> String {
        help_overlay_lines(can_stage, can_stage, &[], 80, Theme::github_dark())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
