//! Rendered terminal rows for sidebar, diff content, and status bars.
//!
//! This module is the row-rendering interface. Submodules own focused pieces of
//! the implementation: sidebar rows, diff rows, intraline emphasis, and wrapping.
//! `ui` owns pane layout and Ratatui widget drawing.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::ask_ai::AskAiResult;
use crate::custom_command::{CustomCommandBinding, CustomCommandResult};
use crate::keybind::{BuiltinAction, KeybindMap};
use crate::model::Changeset;
use crate::theme::Theme;

mod diff;
mod file_summary;
mod intraline;
mod markdown;
mod sidebar;
mod text;

pub(crate) use diff::{diff_layout_counts, diff_lines_until, selected_hunk_header_rows};
pub(crate) use sidebar::{SidebarRowCountsInput, SidebarRowTarget, SidebarRowsInput};
pub(crate) use sidebar::{sidebar_row_counts, sidebar_rows, visible_sidebar_targets};

use text::{color_style, display_width, muted_line, wrap_line, wrap_styled_spans};

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
    cancelling: bool,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(command) = command else {
        return Vec::new();
    };

    let status = if cancelling {
        "Cancelling command"
    } else {
        "Running command"
    };

    wrap_line(
        Line::styled(
            format!(
                "{} {status}: {}",
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
    keybinds: KeybindMap,
    theme: Theme,
) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD);
    let label_style = color_style(theme.muted, background);
    let separator_style = color_style(theme.border, background);

    let move_keys = key_pair(keybinds, BuiltinAction::MoveDown, BuiltinAction::MoveUp);
    let mut hints: Vec<(String, &'static str)> = vec![(
        keybinds.display(BuiltinAction::ToggleFiles),
        if files_panel_visible {
            "hide files"
        } else {
            "show files"
        },
    )];
    if files_panel_visible {
        hints.push(("Tab".to_string(), "focus"));
    }
    if let Some(stage_hint) = stage_hint {
        hints.push((keybinds.display(BuiltinAction::ToggleStaging), stage_hint));
    }
    hints.push((keybinds.display(BuiltinAction::Search), "search"));
    hints.push((move_keys, "move"));
    hints.push((keybinds.display(BuiltinAction::Help), "help"));
    hints.push((keybinds.display(BuiltinAction::Quit), "quit"));

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

fn keybind_key_span(key: &str, style: Style) -> Span<'static> {
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

pub(crate) fn ask_ai_running_keybind_bar_line(keybinds: KeybindMap, theme: Theme) -> Line<'static> {
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
        keybind_key_span(
            &format!("Esc/{}", keybinds.display(BuiltinAction::Quit)),
            key_style,
        ),
        Span::styled(" cancel", label_style),
        Span::styled("  \u{b7}  ", separator_style),
        keybind_key_span("Ctrl-c", key_style),
        Span::styled(" quit", label_style),
    ])
}

pub(crate) fn custom_command_running_keybind_bar_line(
    keybinds: KeybindMap,
    theme: Theme,
) -> Line<'static> {
    let background = theme.background;
    let key_style = color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD);
    let label_style = color_style(theme.muted, background);

    Line::from(vec![
        Span::styled(
            " COMMAND ",
            color_style(theme.on_accent, theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{e0b0}", color_style(theme.accent, background)),
        Span::styled("  ", label_style),
        keybind_key_span(
            &format!("Esc/{}/Ctrl-c", keybinds.display(BuiltinAction::Quit)),
            key_style,
        ),
        Span::styled(" cancel", label_style),
    ])
}

pub(crate) fn ask_ai_output_keybind_bar_line(keybinds: KeybindMap, theme: Theme) -> Line<'static> {
    output_keybind_bar_line(" ASK AI ", true, keybinds, theme)
}

pub(crate) fn custom_command_output_keybind_bar_line(
    keybinds: KeybindMap,
    theme: Theme,
) -> Line<'static> {
    output_keybind_bar_line(" COMMAND ", false, keybinds, theme)
}

fn output_keybind_bar_line(
    tag: &'static str,
    include_copy_hint: bool,
    keybinds: KeybindMap,
    theme: Theme,
) -> Line<'static> {
    let move_keys = key_pair(keybinds, BuiltinAction::MoveDown, BuiltinAction::MoveUp);
    let top_bottom = key_pair(keybinds, BuiltinAction::Top, BuiltinAction::Bottom);
    let close = format!("Esc/{}", keybinds.display(BuiltinAction::Quit));
    let mut hints: Vec<(String, &'static str)> = vec![
        (move_keys, "scroll"),
        ("Ctrl-d/Ctrl-u".to_string(), "page"),
        (top_bottom, "top/bottom"),
    ];
    if include_copy_hint {
        hints.push((keybinds.display(BuiltinAction::CopyFocused), "copy"));
    }
    hints.push((close, "close"));

    tagged_keybind_bar_line(tag, &hints, theme)
}

fn tagged_keybind_bar_line(
    tag: &'static str,
    hints: &[(String, &'static str)],
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

fn key_pair(keybinds: KeybindMap, first: BuiltinAction, second: BuiltinAction) -> String {
    format!("{}/{}", keybinds.display(first), keybinds.display(second))
}

pub(crate) fn help_overlay_lines(
    can_stage: bool,
    can_discard: bool,
    custom_commands: &[CustomCommandBinding],
    keybinds: KeybindMap,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let action_entry =
        |action, description| HelpEntry::text(command_key(keybinds.display(action)), description);
    let mut lines = Vec::new();

    push_help_entries(
        &mut lines,
        "Global",
        &[
            action_entry(BuiltinAction::Help, "help/dismiss"),
            action_entry(BuiltinAction::Quit, "close help or quit"),
            HelpEntry::text(command_key("Ctrl-c"), "quit"),
            action_entry(BuiltinAction::ToggleFiles, "show or hide files"),
            HelpEntry::text(key_names(["Tab", "Left", "Right", "Enter"]), "focus panes"),
            action_entry(BuiltinAction::Search, "search"),
            HelpEntry::text(command_key("Enter"), "apply search"),
            HelpEntry::text(command_key("Esc"), "cancel or clear search"),
            HelpEntry::text(
                action_keys(
                    keybinds,
                    [BuiltinAction::NextMatch, BuiltinAction::PrevMatch],
                ),
                "next or previous match/hunk",
            ),
            HelpEntry::text(
                key_names([keybinds.display(BuiltinAction::Top), "Home".to_string()]),
                "top",
            ),
            HelpEntry::text(
                key_names([keybinds.display(BuiltinAction::Bottom), "End".to_string()]),
                "bottom",
            ),
        ],
        content_width,
        theme,
    );

    push_custom_commands_section(&mut lines, custom_commands, content_width, theme);

    push_help_entries(
        &mut lines,
        "Sidebar",
        &[
            HelpEntry::text(
                action_keys(keybinds, [BuiltinAction::MoveDown, BuiltinAction::MoveUp]),
                "select file",
            ),
            action_entry(
                BuiltinAction::ToggleReviewed,
                "toggle reviewed for selected file",
            ),
            action_entry(BuiltinAction::CopyFocused, "copy selected file path"),
        ],
        content_width,
        theme,
    );

    push_help_entries(
        &mut lines,
        "Diff",
        &[
            HelpEntry::text(
                action_keys(keybinds, [BuiltinAction::MoveDown, BuiltinAction::MoveUp]),
                "scroll row",
            ),
            HelpEntry::text(key_names(["Ctrl-d", "Ctrl-u"]), "page"),
            HelpEntry::text(
                action_keys(
                    keybinds,
                    [BuiltinAction::NextMatch, BuiltinAction::PrevMatch],
                ),
                "next or previous hunk",
            ),
            action_entry(BuiltinAction::CopyFocused, "copy selected hunk diff"),
            action_entry(BuiltinAction::CopyFileDiff, "copy selected file diff"),
        ],
        content_width,
        theme,
    );

    push_help_entries(
        &mut lines,
        "AI",
        &[
            HelpEntry::new(
                command_key(keybinds.display(BuiltinAction::AskAi)),
                [
                    HelpSegment::text("Ask AI about focused file or hunk; "),
                    HelpSegment::command("Enter"),
                    HelpSegment::text(" submit; "),
                    HelpSegment::command("Esc"),
                    HelpSegment::text(" cancel"),
                ],
            ),
            action_entry(
                BuiltinAction::ExplainCode,
                "Explain focused file or hunk with Ask AI",
            ),
            action_entry(
                BuiltinAction::UnpublishedSummary,
                "Summarize unpublished changes with Ask AI",
            ),
        ],
        content_width,
        theme,
    );

    push_help_entries(
        &mut lines,
        "Mouse",
        &[
            HelpEntry::text(command_key("hover"), "focus pane"),
            HelpEntry::text(command_key("click file"), "select file"),
            HelpEntry::text(command_key("click hunk"), "select hunk"),
            HelpEntry::text(command_key("wheel"), "scroll pointed pane"),
            HelpEntry::text(command_key("drag text"), "copy selection"),
        ],
        content_width,
        theme,
    );

    if can_stage || can_discard {
        let mut entries = Vec::new();
        if can_stage {
            entries.push(action_entry(
                BuiltinAction::ToggleStaging,
                "stage/unstage focused file or hunk",
            ));
        }
        if can_discard {
            entries.push(HelpEntry::new(
                command_key(keybinds.display(BuiltinAction::Discard)),
                [
                    HelpSegment::text("discard focused file, folder, or hunk; "),
                    HelpSegment::command("y"),
                    HelpSegment::text(" / "),
                    HelpSegment::command("Enter"),
                    HelpSegment::text(" confirm"),
                ],
            ));
        }
        entries.push(action_entry(
            BuiltinAction::Editor,
            "open selected file in $EDITOR",
        ));
        push_help_entries(&mut lines, "Worktree-only", &entries, content_width, theme);
    } else {
        push_help_section(&mut lines, "Worktree-only", theme);
        push_help_message(
            &mut lines,
            [HelpSegment::muted(
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
    if custom_commands.is_empty() {
        push_help_section(lines, "Custom commands", theme);
        push_help_message(
            lines,
            [HelpSegment::muted("No custom commands configured")],
            content_width,
            theme,
        );
    } else {
        let entries = custom_commands
            .iter()
            .map(|command| {
                HelpEntry::new(
                    command_key(command.key_display()),
                    [
                        HelpSegment::text(command.label()),
                        HelpSegment::muted(format!("  {}", command.command())),
                    ],
                )
            })
            .collect::<Vec<_>>();
        push_help_entries(lines, "Custom commands", &entries, content_width, theme);
    }
}

fn push_help_section(lines: &mut Vec<Line<'static>>, title: &'static str, theme: Theme) {
    if !lines.is_empty() {
        lines.push(Line::styled("", help_style(theme.text, theme)));
    }

    lines.push(Line::styled(title, help_section_style(theme)));
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelpSegment {
    Command(String),
    Text(String),
    Muted(String),
}

impl HelpSegment {
    fn command(text: impl Into<String>) -> Self {
        Self::Command(text.into())
    }

    fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    fn muted(text: impl Into<String>) -> Self {
        Self::Muted(text.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HelpEntry {
    keys: Vec<HelpSegment>,
    description: Vec<HelpSegment>,
}

impl HelpEntry {
    fn new(keys: impl Into<Vec<HelpSegment>>, description: impl Into<Vec<HelpSegment>>) -> Self {
        Self {
            keys: keys.into(),
            description: description.into(),
        }
    }

    fn text(keys: impl Into<Vec<HelpSegment>>, description: impl Into<String>) -> Self {
        Self::new(keys, [HelpSegment::text(description)])
    }
}

const HELP_COLUMN_GAP: usize = 2;
const HELP_MIN_DESCRIPTION_WIDTH: usize = 18;

fn push_help_entries(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    entries: &[HelpEntry],
    content_width: usize,
    theme: Theme,
) {
    push_help_section(lines, title, theme);
    let key_width = entries
        .iter()
        .map(|entry| segments_width(&entry.keys))
        .max()
        .unwrap_or(0);

    let align_columns = content_width >= key_width + HELP_COLUMN_GAP + HELP_MIN_DESCRIPTION_WIDTH;

    for entry in entries {
        push_help_entry(lines, entry, key_width, align_columns, content_width, theme);
    }
}

fn push_help_entry(
    lines: &mut Vec<Line<'static>>,
    entry: &HelpEntry,
    key_width: usize,
    align_columns: bool,
    content_width: usize,
    theme: Theme,
) {
    if !align_columns {
        let mut spans = help_spans(&entry.keys, theme);
        spans.push(Span::styled(" ", help_style(theme.text, theme)));
        spans.extend(help_spans(&entry.description, theme));
        lines.extend(wrap_line(Line::from(spans), content_width));
        return;
    }

    let desc_column = key_width + HELP_COLUMN_GAP;
    let desc_width = content_width.saturating_sub(desc_column).max(1);
    let mut desc_rows = wrap_styled_spans(help_spans(&entry.description, theme), desc_width);
    if desc_rows.is_empty() {
        desc_rows.push(Vec::new());
    }

    for (index, desc_spans) in desc_rows.into_iter().enumerate() {
        let mut spans = Vec::new();
        if index == 0 {
            spans.extend(padded_help_key_spans(&entry.keys, key_width, theme));
            spans.push(Span::styled(
                " ".repeat(HELP_COLUMN_GAP),
                help_style(theme.text, theme),
            ));
        } else {
            spans.push(Span::styled(
                " ".repeat(desc_column),
                help_style(theme.text, theme),
            ));
        }
        spans.extend(desc_spans);
        lines.push(Line::from(spans));
    }
}

fn push_help_message(
    lines: &mut Vec<Line<'static>>,
    segments: impl Into<Vec<HelpSegment>>,
    content_width: usize,
    theme: Theme,
) {
    let segments = segments.into();
    lines.extend(wrap_line(
        Line::from(help_spans(&segments, theme)),
        content_width,
    ));
}

fn help_spans(segments: &[HelpSegment], theme: Theme) -> Vec<Span<'static>> {
    segments
        .iter()
        .map(|segment| match segment {
            HelpSegment::Command(text) => Span::styled(text.clone(), help_command_style(theme)),
            HelpSegment::Text(text) => Span::styled(text.clone(), help_style(theme.text, theme)),
            HelpSegment::Muted(text) => Span::styled(text.clone(), help_style(theme.muted, theme)),
        })
        .collect()
}

fn padded_help_key_spans(
    key_segments: &[HelpSegment],
    key_width: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    let mut spans = help_spans(key_segments, theme);
    let padding = key_width.saturating_sub(segments_width(key_segments));
    if padding > 0 {
        spans.push(Span::styled(
            " ".repeat(padding),
            help_style(theme.text, theme),
        ));
    }
    spans
}

fn segments_width(segments: &[HelpSegment]) -> usize {
    segments
        .iter()
        .map(|segment| match segment {
            HelpSegment::Command(text) | HelpSegment::Text(text) | HelpSegment::Muted(text) => {
                display_width(text)
            }
        })
        .sum()
}

fn key_names(names: impl IntoIterator<Item = impl Into<String>>) -> Vec<HelpSegment> {
    let mut segments = Vec::new();
    for (index, name) in names.into_iter().enumerate() {
        if index > 0 {
            segments.push(HelpSegment::text(" / "));
        }
        segments.push(HelpSegment::command(name));
    }
    segments
}

fn command_key(key: impl Into<String>) -> [HelpSegment; 1] {
    [HelpSegment::command(key)]
}

fn action_keys(
    keybinds: KeybindMap,
    actions: impl IntoIterator<Item = BuiltinAction>,
) -> Vec<HelpSegment> {
    key_names(actions.into_iter().map(|action| keybinds.display(action)))
}

fn help_command_style(theme: Theme) -> Style {
    help_style(theme.accent, theme).add_modifier(Modifier::BOLD)
}

fn help_section_style(theme: Theme) -> Style {
    help_style(theme.text, theme).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
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
            "AI",
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

        assert!(worktree_help.contains("Space  stage/unstage focused file or hunk"));
        assert!(worktree_help.contains("d      discard focused file, folder, or hunk"));
        assert!(worktree_help.contains("e      open selected file in $EDITOR"));
        assert!(pr_help.contains("Worktree actions unavailable in PR mode"));
        assert!(!pr_help.contains("stage/unstage focused file or hunk"));
        assert!(!pr_help.contains("discard focused file, folder, or hunk"));
    }

    #[test]
    fn help_overlay_groups_ai_actions_separately() {
        let help = help_text(true);

        assert!(help.contains("AI"));
        assert!(help.contains("a  Ask AI about focused file or hunk"));
        assert!(help.contains("x  Explain focused file or hunk with Ask AI"));
        assert!(help.contains("u  Summarize unpublished changes with Ask AI"));
    }

    #[test]
    fn help_overlay_lists_review_toggle_in_sidebar_section() {
        let worktree_help = help_text(true);
        let pr_help = help_text(false);

        assert!(worktree_help.contains("r      toggle reviewed for selected file"));
        assert!(pr_help.contains("r      toggle reviewed for selected file"));
    }

    #[test]
    fn help_overlay_uses_distinct_header_and_key_colors() {
        let theme = Theme::github_dark();

        assert_eq!(help_section_style(theme).fg, Some(theme.text));
        assert_eq!(help_command_style(theme).fg, Some(theme.accent));
        assert_ne!(help_section_style(theme).fg, help_command_style(theme).fg);
        assert!(
            help_section_style(theme)
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn help_overlay_aligns_descriptions_by_section() {
        let lines = help_lines(true, 80);
        let texts = lines.iter().map(line_text).collect::<Vec<_>>();

        let help_column = column_of(&texts, "help/dismiss");
        assert_eq!(column_of(&texts, "close help or quit"), help_column);
        assert_eq!(column_of(&texts, "focus panes"), help_column);

        let diff_column = column_of(&texts, "scroll row");
        assert_eq!(column_of(&texts, "page"), diff_column);
        assert_eq!(column_of(&texts, "copy selected file diff"), diff_column);
    }

    #[test]
    fn help_overlay_aligns_custom_commands() {
        let commands = [
            custom_command("P", "publish", "git push"),
            custom_command("B", "build release", "cargo build --release"),
        ];
        let lines = help_overlay_lines(
            true,
            true,
            &commands,
            KeybindMap::defaults(),
            80,
            Theme::github_dark(),
        );
        let texts = lines.iter().map(line_text).collect::<Vec<_>>();

        let label_column = column_of(&texts, "publish");
        assert_eq!(column_of(&texts, "build release"), label_column);
        assert!(
            texts
                .iter()
                .any(|line| line.contains("P  publish  git push"))
        );
    }

    #[test]
    fn help_overlay_wraps_cleanly_in_narrow_widths() {
        let lines = help_overlay_lines(
            true,
            true,
            &[],
            KeybindMap::defaults(),
            24,
            Theme::github_dark(),
        );
        let help = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        for line in &lines {
            assert!(
                line.width() <= 24,
                "line exceeded content width: {:?}",
                line_text(line)
            );
        }
        assert!(help.contains("help/dismiss"));
        assert!(help.contains("close help or quit"));
        assert!(help.contains("stage/unstage"));
        assert!(help.contains("focused file or hunk"));
    }

    #[test]
    fn keybind_bar_colors_key_tokens_with_accent_fill() {
        let theme = Theme::github_dark();
        let line = keybind_bar_line(true, Some("stage file"), KeybindMap::defaults(), theme);
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
        help_lines(can_stage, 80)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn help_lines(can_stage: bool, content_width: usize) -> Vec<Line<'static>> {
        help_overlay_lines(
            can_stage,
            can_stage,
            &[],
            KeybindMap::defaults(),
            content_width,
            Theme::github_dark(),
        )
    }

    fn custom_command(key: &str, label: &str, command: &str) -> CustomCommandBinding {
        CustomCommandBinding::new(
            crate::custom_command::CommandKey::parse(key).unwrap(),
            label.to_string(),
            command.to_string(),
        )
    }

    fn column_of(lines: &[String], needle: &str) -> usize {
        lines
            .iter()
            .find_map(|line| line.find(needle))
            .unwrap_or_else(|| panic!("missing {needle:?} in {lines:#?}"))
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
