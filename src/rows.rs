//! Rendered terminal rows for sidebar, diff content, and status bars.
//!
//! This module owns row construction, wrapping, gutters, syntax advancement,
//! and display labels. `ui` owns pane layout and Ratatui widget drawing.

use std::str::Lines;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{
    Changeset, DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffSource, FileStage, FileStatus,
    SourceSnapshot,
};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

const SIDEBAR_STAGE_GUTTER_WIDTH: usize = 8;
const SIDEBAR_REVIEW_GUTTER_WIDTH: usize = 4;
const DIFF_GUTTER_WIDTH: usize = 11;
pub(crate) const DIFF_PREFETCH_ROWS: usize = 120;
const RAIL_MARKER: &str = "▌";
const NO_TRACKED_CHANGES: &str = "No tracked changes";
const NO_DIFF_MESSAGE: &str = "No diff to review. Make a tracked change, then run chunk diff.";
const NO_BRANCH_CHANGES: &str = "No branch changes";
const NO_PR_DIFF_MESSAGE: &str = "No diff to review. Current branch has no changes against base.";

pub(crate) struct SidebarRowsInput<'a> {
    pub(crate) files: &'a [DiffFile],
    pub(crate) source: &'a DiffSource,
    pub(crate) selected_file_index: usize,
    pub(crate) sidebar_scroll: usize,
    pub(crate) row_counts: &'a [usize],
    pub(crate) content_width: usize,
    pub(crate) visible_height: usize,
    pub(crate) theme: Theme,
}

pub(crate) struct RenderedSidebarRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) row_records: Vec<SidebarRowRecord>,
    pub(crate) sidebar_scroll: usize,
}

pub(crate) struct SidebarRowRecord {
    pub(crate) index: usize,
    pub(crate) row_count: usize,
}

pub(crate) struct RenderedRows {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) complete: bool,
}

impl RenderedRows {
    fn complete(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            complete: true,
        }
    }

    fn partial(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            complete: false,
        }
    }
}

struct DiffSyntaxHighlighter<'a> {
    highlighter: SyntaxHighlighter,
    source_lines: Option<Lines<'a>>,
    next_line: u32,
}

pub(crate) fn sidebar_rows(input: SidebarRowsInput<'_>) -> RenderedSidebarRows {
    if input.files.is_empty() {
        return RenderedSidebarRows {
            lines: vec![muted_line(empty_sidebar_message(input.source), input.theme)],
            row_records: Vec::new(),
            sidebar_scroll: 0,
        };
    }

    let can_stage = input.source.can_stage();
    let sidebar_scroll = visible_sidebar_scroll(
        input.row_counts,
        input.sidebar_scroll,
        input.selected_file_index,
        input.files.len(),
        input.visible_height,
    );
    let selected_file_index = input.selected_file_index.min(input.files.len() - 1);

    let mut lines = Vec::new();
    let mut row_records = Vec::new();
    for (index, file) in input.files.iter().enumerate().skip(sidebar_scroll) {
        let entry_lines = render_file_entry(
            index,
            file,
            selected_file_index,
            input.content_width,
            can_stage,
            input.theme,
        );
        let visible_rows = entry_lines
            .len()
            .min(input.visible_height.saturating_sub(lines.len()));
        if visible_rows == 0 {
            break;
        }

        row_records.push(SidebarRowRecord {
            index,
            row_count: visible_rows,
        });
        lines.extend(entry_lines.into_iter().take(visible_rows));
        if lines.len() >= input.visible_height {
            break;
        }
    }

    RenderedSidebarRows {
        lines,
        row_records,
        sidebar_scroll,
    }
}

pub(crate) fn sidebar_row_counts(
    files: &[DiffFile],
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            render_file_entry(index, file, usize::MAX, content_width, can_stage, theme).len()
        })
        .collect()
}

pub(crate) fn diff_lines_until(
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
    target_rows: usize,
) -> RenderedRows {
    let mut lines = wrap_line(
        render_file_header(file, content_width, can_stage, theme),
        content_width,
    );

    if file.binary {
        lines.extend(wrap_line(
            muted_line("Binary file changed", theme),
            content_width,
        ));
        return RenderedRows::complete(lines);
    }

    if file.hunks.is_empty() {
        lines.extend(wrap_line(
            muted_line("File changed without textual hunks", theme),
            content_width,
        ));
        return RenderedRows::complete(lines);
    }

    let mut old_highlighter =
        DiffSyntaxHighlighter::new(diff_old_path(file), &file.old_source, theme);
    let mut new_highlighter =
        DiffSyntaxHighlighter::new(diff_new_path(file), &file.new_source, theme);

    for hunk in &file.hunks {
        if !push_hunk_lines_until(
            &mut lines,
            hunk,
            &mut old_highlighter,
            &mut new_highlighter,
            content_width,
            theme,
            target_rows,
        ) {
            return RenderedRows::partial(lines);
        }
    }

    RenderedRows::complete(lines)
}

pub(crate) fn no_diff_lines(
    source: &DiffSource,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    wrap_line(muted_line(no_diff_message(source), theme), content_width)
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

pub(crate) fn keybind_bar_line(
    files_panel_visible: bool,
    can_stage: bool,
    theme: Theme,
) -> Line<'static> {
    let mut hints = vec![if files_panel_visible {
        "[f] hide files"
    } else {
        "[f] show files"
    }];

    if files_panel_visible {
        hints.push("[Tab] switch focus");
        if can_stage {
            hints.push("[Space] stage");
        }
    }
    hints.push("[j/k] move");
    hints.push("[Ctrl-d/u] scroll");
    hints.push("[q] quit");

    Line::styled(hints.join("  |  "), Style::default().fg(theme.muted))
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

fn visible_sidebar_scroll(
    row_counts: &[usize],
    sidebar_scroll: usize,
    selected_file_index: usize,
    file_count: usize,
    visible_height: usize,
) -> usize {
    if file_count == 0 {
        return 0;
    }

    let selected_index = selected_file_index.min(file_count - 1);
    let scroll = sidebar_scroll.min(file_count - 1);

    if selected_index < scroll {
        return selected_index;
    }

    if sidebar_selection_visible(row_counts, scroll, selected_index, visible_height) {
        scroll
    } else {
        sidebar_scroll_for_selected(row_counts, selected_index, visible_height)
    }
}

fn sidebar_selection_visible(
    row_counts: &[usize],
    scroll: usize,
    selected_index: usize,
    visible_height: usize,
) -> bool {
    if selected_index < scroll || selected_index >= row_counts.len() {
        return false;
    }

    let visible_height = visible_height.max(1);
    let rows_before_selected: usize = row_counts[scroll..selected_index].iter().sum();
    if rows_before_selected >= visible_height {
        return false;
    }

    rows_before_selected == 0 || rows_before_selected + row_counts[selected_index] <= visible_height
}

fn sidebar_scroll_for_selected(
    row_counts: &[usize],
    selected_index: usize,
    visible_height: usize,
) -> usize {
    let visible_height = visible_height.max(1);
    let mut scroll = selected_index;
    let mut rows = row_counts.get(selected_index).copied().unwrap_or(1);

    while scroll > 0 {
        let previous_rows = row_counts.get(scroll - 1).copied().unwrap_or(1);
        if rows + previous_rows > visible_height {
            break;
        }

        scroll -= 1;
        rows += previous_rows;
    }

    scroll
}

fn render_file_entry(
    index: usize,
    file: &DiffFile,
    selected_index: usize,
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Vec<Line<'static>> {
    let selected = index == selected_index;
    let background = if selected {
        theme.selected
    } else {
        theme.background
    };
    let base = color_style(theme.text, background);
    let marker_style = if selected {
        color_style(theme.accent, background)
    } else {
        base
    };
    let status_style = color_style(status_color(file.status, theme), background);
    let label = sidebar_file_label(file);
    let stats = format_file_stats(file);
    let stats_width = display_width(&stats);
    let gutter_width = sidebar_gutter_width(can_stage);
    let used_width = gutter_width + display_width(&label) + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let rail = if selected { RAIL_MARKER } else { " " };

    let mut prefix = vec![Span::styled(rail, marker_style), Span::styled(" ", base)];
    if can_stage {
        let stage = stage_display(file.stage, background, theme);
        prefix.push(Span::styled(stage.checkbox, stage.style));
        prefix.push(Span::styled(" ", base));
    }
    prefix.push(Span::styled(file.status.marker().to_string(), status_style));
    prefix.push(Span::styled(" ", base));
    let mut content = vec![Span::styled(label, base), Span::styled(padding, base)];
    push_stat_spans(&mut content, file, background, theme);

    if content_width <= gutter_width {
        let mut spans = prefix;
        spans.extend(content);
        return wrap_line(Line::from(spans), content_width);
    }

    wrap_sidebar_content(
        prefix,
        continuation_prefix(rail, marker_style, base, gutter_width),
        content,
        content_width,
        gutter_width,
    )
}

fn wrap_sidebar_content(
    first_prefix: Vec<Span<'static>>,
    continuation_prefix: Vec<Span<'static>>,
    content: Vec<Span<'static>>,
    content_width: usize,
    gutter_width: usize,
) -> Vec<Line<'static>> {
    wrap_styled_spans(content, content_width.saturating_sub(gutter_width))
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let mut spans = if index == 0 {
                first_prefix.clone()
            } else {
                continuation_prefix.clone()
            };
            spans.extend(row);
            Line::from(spans)
        })
        .collect()
}

fn continuation_prefix(
    rail: &str,
    marker_style: Style,
    base: Style,
    gutter_width: usize,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(rail.to_string(), marker_style),
        Span::styled(" ".repeat(gutter_width.saturating_sub(1)), base),
    ]
}

fn push_hunk_lines_until(
    lines: &mut Vec<Line<'static>>,
    hunk: &DiffHunk,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
    content_width: usize,
    theme: Theme,
    target_rows: usize,
) -> bool {
    old_highlighter.advance_to(hunk.old_start);
    new_highlighter.advance_to(hunk.new_start);

    lines.extend(wrap_line(
        hunk_header_line(&hunk.header, theme),
        content_width,
    ));
    if lines.len() >= target_rows {
        return false;
    }

    for line in &hunk.lines {
        lines.extend(diff_line(
            line,
            old_highlighter,
            new_highlighter,
            content_width,
            theme,
        ));
        if lines.len() >= target_rows {
            return false;
        }
    }

    true
}

fn diff_old_path(file: &DiffFile) -> &str {
    path_or_display(&file.old_path, file)
}

fn diff_new_path(file: &DiffFile) -> &str {
    path_or_display(&file.path, file)
}

fn path_or_display<'a>(path: &'a str, file: &'a DiffFile) -> &'a str {
    if path.is_empty() {
        file.display_path()
    } else {
        path
    }
}

fn muted_line(text: impl Into<String>, theme: Theme) -> Line<'static> {
    Line::styled(text.into(), color_style(theme.muted, theme.background))
}

fn hunk_header_line(header: &str, theme: Theme) -> Line<'static> {
    Line::styled(
        format!(" {header}"),
        color_style(theme.muted, theme.background_alt).add_modifier(Modifier::BOLD),
    )
}

fn diff_line(
    line: &DiffLine,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
    content_width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let style = diff_line_style(line.kind, theme);
    let number_style = color_style(theme.line_number_fg, theme.line_number_bg);
    let content_spans = highlight_diff_content(
        line.kind,
        &line.content,
        style.content,
        old_highlighter,
        new_highlighter,
    );

    wrap_diff_line(
        line.old_line,
        line.new_line,
        content_spans,
        content_width,
        style,
        number_style,
    )
}

fn wrap_diff_line(
    old_line: Option<u32>,
    new_line: Option<u32>,
    content_spans: Vec<Span<'static>>,
    content_width: usize,
    style: DiffLineStyle,
    number_style: Style,
) -> Vec<Line<'static>> {
    let content_rows = wrap_styled_spans(content_spans, diff_content_width(content_width));
    content_rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let mut spans = if index == 0 {
                diff_gutter_spans(old_line, new_line, style, number_style)
            } else {
                continuation_gutter_spans(style, number_style)
            };
            spans.extend(row);
            Line::from(spans)
        })
        .collect()
}

fn diff_gutter_spans(
    old_line: Option<u32>,
    new_line: Option<u32>,
    style: DiffLineStyle,
    number_style: Style,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(RAIL_MARKER, style.rail),
        Span::styled(format_line_number(old_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(format_line_number(new_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(style.marker, style.content),
        Span::styled(" ", style.content),
    ]
}

fn continuation_gutter_spans(style: DiffLineStyle, number_style: Style) -> Vec<Span<'static>> {
    vec![
        Span::styled(RAIL_MARKER, style.rail),
        Span::styled("   ", number_style),
        Span::styled(" ", number_style),
        Span::styled("   ", number_style),
        Span::styled(" ", number_style),
        Span::styled(" ", style.content),
        Span::styled(" ", style.content),
    ]
}

fn highlight_diff_content(
    kind: DiffLineKind,
    content: &str,
    content_style: Style,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
) -> Vec<Span<'static>> {
    match kind {
        DiffLineKind::Added => new_highlighter.highlight_line(content, content_style),
        DiffLineKind::Removed => old_highlighter.highlight_line(content, content_style),
        DiffLineKind::Context => {
            highlight_context_content(content, content_style, old_highlighter, new_highlighter)
        }
        DiffLineKind::Meta => vec![Span::styled(expand_tabs(content), content_style)],
    }
}

fn highlight_context_content(
    content: &str,
    content_style: Style,
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
) -> Vec<Span<'static>> {
    if new_highlighter.is_enabled() {
        let spans = new_highlighter.highlight_line(content, content_style);
        old_highlighter.advance_line(content);
        return spans;
    }

    let spans = old_highlighter.highlight_line(content, content_style);
    new_highlighter.advance_line(content);
    spans
}

impl<'a> DiffSyntaxHighlighter<'a> {
    fn new(path: &str, source: &'a SourceSnapshot, theme: Theme) -> Self {
        let highlighter = match source {
            SourceSnapshot::Unavailable => SyntaxHighlighter::disabled(),
            SourceSnapshot::Loaded(_) | SourceSnapshot::Unloaded => {
                SyntaxHighlighter::for_path(path, theme.syntax)
            }
        };

        Self {
            highlighter,
            source_lines: source.as_str().map(str::lines),
            next_line: 1,
        }
    }

    fn is_enabled(&self) -> bool {
        self.highlighter.is_enabled()
    }

    fn advance_to(&mut self, target_line: u32) {
        while self.next_line < target_line && self.advance_source_line() {}

        if self.next_line < target_line {
            self.next_line = target_line;
        }
    }

    fn highlight_line(&mut self, content: &str, base_style: Style) -> Vec<Span<'static>> {
        let expanded_content = expand_tabs(content);
        let spans = self
            .highlighter
            .highlight_line(&expanded_content, base_style);
        self.advance_position();
        spans
    }

    fn advance_line(&mut self, content: &str) {
        self.advance_highlighter_line(content);
        self.advance_position();
    }

    fn advance_source_line(&mut self) -> bool {
        let Some(content) = self.take_source_line() else {
            return false;
        };

        self.advance_highlighter_line(content);
        self.next_line += 1;
        true
    }

    fn advance_highlighter_line(&mut self, content: &str) {
        let expanded_content = expand_tabs(content);
        self.highlighter.advance_line(&expanded_content);
    }

    fn advance_position(&mut self) {
        let _ = self.take_source_line();
        self.next_line += 1;
    }

    fn take_source_line(&mut self) -> Option<&'a str> {
        self.source_lines.as_mut().and_then(Iterator::next)
    }
}

#[derive(Debug, Clone, Copy)]
struct DiffLineStyle {
    marker: &'static str,
    content: Style,
    rail: Style,
}

fn diff_line_style(kind: DiffLineKind, theme: Theme) -> DiffLineStyle {
    let (marker, text_color, background, rail_color) = match kind {
        DiffLineKind::Context => (" ", theme.text, theme.context_bg, theme.line_number_fg),
        DiffLineKind::Added => ("+", theme.added, theme.added_bg, theme.added),
        DiffLineKind::Removed => ("-", theme.removed, theme.removed_bg, theme.removed),
        DiffLineKind::Meta => (" ", theme.muted, theme.context_bg, theme.line_number_fg),
    };

    DiffLineStyle {
        marker,
        content: color_style(text_color, background),
        rail: color_style(rail_color, background),
    }
}

fn wrap_line(line: Line<'static>, content_width: usize) -> Vec<Line<'static>> {
    let style = line.style;
    let alignment = line.alignment;
    wrap_styled_spans(line.spans, content_width.max(1))
        .into_iter()
        .map(|spans| Line {
            style,
            alignment,
            spans,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StyledChar {
    value: char,
    style: Style,
    width: usize,
}

fn wrap_styled_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Vec<Span<'static>>> {
    let max_width = max_width.max(1);
    let chars = styled_chars(spans);
    if chars.is_empty() {
        return vec![Vec::new()];
    }

    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = wrapped_row_end(&chars, start, max_width);
        rows.push(chars_to_spans(&chars[start..end]));
        start = end;
    }

    rows
}

fn styled_chars(spans: Vec<Span<'static>>) -> Vec<StyledChar> {
    let mut chars = Vec::new();
    for span in spans {
        chars.extend(span.content.chars().map(|value| StyledChar {
            value,
            style: span.style,
            width: char_display_width(value),
        }));
    }
    chars
}

fn wrapped_row_end(chars: &[StyledChar], start: usize, max_width: usize) -> usize {
    let mut width = 0;
    let mut index = start;
    let mut last_break = None;

    while index < chars.len() {
        let char_width = chars[index].width;
        if width > 0 && width + char_width > max_width {
            break;
        }
        if width == 0 && char_width > max_width {
            return index + 1;
        }

        width += char_width;
        index += 1;

        if chars[index - 1].value.is_whitespace() {
            last_break = Some(index);
        }

        if width >= max_width {
            break;
        }
    }

    if index >= chars.len() {
        return chars.len();
    }

    last_break
        .filter(|break_index| *break_index > start)
        .unwrap_or(index.max(start + 1))
}

fn chars_to_spans(chars: &[StyledChar]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    for character in chars {
        match spans.last_mut() {
            Some(span) if span.style == character.style => {
                span.content.to_mut().push(character.value);
            }
            _ => spans.push(Span::styled(character.value.to_string(), character.style)),
        }
    }

    spans
}

fn diff_content_width(content_width: usize) -> usize {
    content_width.saturating_sub(DIFF_GUTTER_WIDTH).max(1)
}

fn char_display_width(value: char) -> usize {
    Span::raw(value.to_string()).width()
}

fn format_line_number(line: Option<u32>) -> String {
    line.map_or_else(|| "   ".to_string(), |line| format!("{line:<3}"))
}

fn status_color(status: FileStatus, theme: Theme) -> Color {
    match status {
        FileStatus::Added => theme.file_new,
        FileStatus::Deleted => theme.file_deleted,
        FileStatus::Modified => theme.file_modified,
        FileStatus::Renamed | FileStatus::Copied => theme.file_renamed,
    }
}

fn render_file_header(
    file: &DiffFile,
    content_width: usize,
    can_stage: bool,
    theme: Theme,
) -> Line<'static> {
    let label = file_header_label(file);
    let suffix = file_status_suffix(file.status);
    let stage = can_stage.then(|| stage_display(file.stage, theme.background, theme));
    let stats = format_file_stats(file);
    let stats_width = display_width(&stats);
    let stage_width = stage
        .as_ref()
        .map_or(0, |stage| display_width(stage.suffix));
    let used_width = display_width(&label) + display_width(suffix) + stage_width + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let style = color_style(theme.text, theme.background);
    let muted_style = color_style(theme.muted, theme.background);

    let mut spans = vec![
        Span::styled(label, style),
        Span::styled(suffix.to_string(), muted_style),
    ];
    if let Some(stage) = stage {
        spans.push(Span::styled(stage.suffix.to_string(), stage.style));
    }
    spans.push(Span::styled(padding, style));
    push_stat_spans(&mut spans, file, theme.background, theme);
    Line::from(spans)
}

fn sidebar_gutter_width(can_stage: bool) -> usize {
    if can_stage {
        SIDEBAR_STAGE_GUTTER_WIDTH
    } else {
        SIDEBAR_REVIEW_GUTTER_WIDTH
    }
}

fn empty_sidebar_message(source: &DiffSource) -> &'static str {
    if source.can_stage() {
        NO_TRACKED_CHANGES
    } else {
        NO_BRANCH_CHANGES
    }
}

fn no_diff_message(source: &DiffSource) -> &'static str {
    if source.can_stage() {
        NO_DIFF_MESSAGE
    } else {
        NO_PR_DIFF_MESSAGE
    }
}

fn push_stat_spans(
    spans: &mut Vec<Span<'static>>,
    file: &DiffFile,
    background: Color,
    theme: Theme,
) {
    let has_additions = file.additions > 0;
    let has_deletions = file.deletions > 0;

    if has_additions {
        spans.push(Span::styled(
            format!("+{}", file.additions),
            color_style(theme.added, background),
        ));
    }

    if has_additions && has_deletions {
        spans.push(Span::styled(" ", Style::default().bg(background)));
    }

    if has_deletions {
        spans.push(Span::styled(
            format!("-{}", file.deletions),
            color_style(theme.removed, background),
        ));
    }
}

fn file_header_label(file: &DiffFile) -> String {
    if has_path_change_label(file) {
        format!("{} -> {}", file.old_path, file.path)
    } else {
        file.display_path().to_string()
    }
}

fn sidebar_file_label(file: &DiffFile) -> String {
    if has_path_change_label(file) {
        format!("{} -> {}", basename(&file.old_path), basename(&file.path))
    } else {
        basename(file.display_path()).to_string()
    }
}

fn has_path_change_label(file: &DiffFile) -> bool {
    matches!(file.status, FileStatus::Renamed | FileStatus::Copied) && file.old_path != file.path
}

fn file_status_suffix(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => " (new)",
        FileStatus::Deleted => " (deleted)",
        FileStatus::Copied => " (copied)",
        FileStatus::Renamed | FileStatus::Modified => "",
    }
}

struct StageDisplay {
    checkbox: &'static str,
    suffix: &'static str,
    style: Style,
}

fn stage_display(stage: FileStage, background: Color, theme: Theme) -> StageDisplay {
    match stage {
        FileStage::Unstaged => StageDisplay {
            checkbox: "[ ]",
            suffix: " [unstaged]",
            style: color_style(theme.muted, background),
        },
        FileStage::Staged => StageDisplay {
            checkbox: "[x]",
            suffix: " [staged]",
            style: color_style(theme.added, background).add_modifier(Modifier::BOLD),
        },
        FileStage::Mixed => StageDisplay {
            checkbox: "[-]",
            suffix: " [mixed]",
            style: color_style(theme.accent, background).add_modifier(Modifier::BOLD),
        },
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn format_file_stats(file: &DiffFile) -> String {
    match (file.additions, file.deletions) {
        (0, 0) => String::new(),
        (additions, 0) => format!("+{additions}"),
        (0, deletions) => format!("-{deletions}"),
        (additions, deletions) => format!("+{additions} -{deletions}"),
    }
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "  ")
}

fn display_width(text: &str) -> usize {
    Span::raw(text.to_string()).width()
}

fn padding_before_stats(content_width: usize, used_width: usize, stats_width: usize) -> String {
    if stats_width == 0 {
        String::new()
    } else {
        " ".repeat(content_width.saturating_sub(used_width).max(1))
    }
}

fn color_style(foreground: Color, background: Color) -> Style {
    Style::default().fg(foreground).bg(background)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_added_removed_context_and_meta_content() {
        for kind in [
            DiffLineKind::Added,
            DiffLineKind::Removed,
            DiffLineKind::Context,
            DiffLineKind::Meta,
        ] {
            let file =
                diff_file_with_line(kind, "alpha beta gamma delta epsilon zeta eta theta iota");
            let content_width = DIFF_GUTTER_WIDTH + 12;
            let lines =
                diff_lines_until(&file, content_width, Theme::github_dark(), true, usize::MAX)
                    .lines;
            let diff_rows: Vec<&Line<'_>> = lines
                .iter()
                .filter(|line| line_text(line).starts_with(RAIL_MARKER))
                .collect();

            assert!(diff_rows.len() > 1, "{kind:?} did not wrap");
            assert!(
                diff_rows.iter().all(|line| line.width() <= content_width),
                "{kind:?} rendered wider than the diff pane"
            );
        }
    }

    #[test]
    fn continuation_rows_align_under_diff_content() {
        let file = diff_file_with_line(
            DiffLineKind::Added,
            "alpha beta gamma delta epsilon zeta eta theta iota",
        );
        let content_width = DIFF_GUTTER_WIDTH + 12;
        let lines =
            diff_lines_until(&file, content_width, Theme::github_dark(), true, usize::MAX).lines;
        let diff_rows: Vec<&Line<'_>> = lines
            .iter()
            .filter(|line| line_text(line).starts_with(RAIL_MARKER))
            .collect();

        let first_prefix = line_prefix(diff_rows[0], DIFF_GUTTER_WIDTH);
        assert!(first_prefix.contains('+'));

        let continuation_prefix = line_prefix(diff_rows[1], DIFF_GUTTER_WIDTH);
        assert_eq!(
            continuation_prefix,
            format!("{RAIL_MARKER}{}", " ".repeat(DIFF_GUTTER_WIDTH - 1))
        );

        let continuation_content = line_suffix(diff_rows[1], DIFF_GUTTER_WIDTH);
        assert!(!continuation_content.is_empty());
        assert!(!continuation_content.starts_with('+'));
    }

    #[test]
    fn vue_hunks_use_source_context_before_the_hunk() {
        let theme = Theme::github_dark();
        let changed_line = "const getCurrentCursorPosition = () =>";
        let file = DiffFile {
            id: "0".to_string(),
            old_path: "src/App.vue".to_string(),
            path: "src/App.vue".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::loaded(
                "<template>\n</template>\n<script setup lang=\"ts\">\n".to_string(),
            ),
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: 1,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: "@@ -4 +4 @@".to_string(),
                old_start: 4,
                old_lines: 0,
                new_start: 4,
                new_lines: 1,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(4),
                    content: changed_line.to_string(),
                }],
            }],
            binary: false,
        };

        let lines = diff_lines_until(&file, DIFF_GUTTER_WIDTH + 80, theme, true, usize::MAX).lines;
        let added_line = lines
            .iter()
            .find(|line| line_text(line).contains(changed_line))
            .expect("changed Vue script line should render");

        assert!(
            added_line
                .spans
                .iter()
                .any(|span| span.content.contains("const")
                    && span.style.fg == Some(theme.syntax.keyword)),
            "{:?}",
            added_line.spans
        );
    }

    #[test]
    fn wrapped_spans_preserve_styles_across_rows() {
        let red = Style::default().fg(Color::Red);
        let blue = Style::default().fg(Color::Blue);
        let rows = wrap_styled_spans(
            vec![Span::styled("abcdef", red), Span::styled("gh", blue)],
            4,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(spans_text(&rows[0]), "abcd");
        assert_eq!(spans_text(&rows[1]), "efgh");
        assert_eq!(rows[1][0].style, red);
        assert_eq!(rows[1][1].style, blue);
    }

    #[test]
    fn sidebar_entries_wrap_to_content_width() {
        let files = vec![diff_file_with_path(
            "src/components/extremely_long_file_name_component.rs",
        )];
        let content_width = 16;
        let row_counts = sidebar_row_counts(&files, content_width, true, Theme::github_dark());
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            source: &DiffSource::Worktree,
            selected_file_index: 0,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width,
            visible_height: 8,
            theme: Theme::github_dark(),
        });

        assert!(rows.lines.len() > 1);
        assert!(rows.lines.iter().all(|line| line.width() <= content_width));
        assert_eq!(rows.row_records.len(), 1);
        assert_eq!(rows.row_records[0].index, 0);
        assert_eq!(rows.row_records[0].row_count, rows.lines.len());
    }

    #[test]
    fn sidebar_scroll_accounts_for_wrapped_rows() {
        let files = vec![
            diff_file_with_path("first_extremely_long_file_name.rs"),
            diff_file_with_path("second_extremely_long_file_name.rs"),
        ];
        let row_counts = sidebar_row_counts(&files, 14, true, Theme::github_dark());
        let rows = sidebar_rows(SidebarRowsInput {
            files: &files,
            source: &DiffSource::Worktree,
            selected_file_index: 1,
            sidebar_scroll: 0,
            row_counts: &row_counts,
            content_width: 14,
            visible_height: 2,
            theme: Theme::github_dark(),
        });

        assert_eq!(rows.sidebar_scroll, 1);
        assert_eq!(rows.lines.len(), 2);
        assert_eq!(rows.row_records.len(), 1);
        assert_eq!(rows.row_records[0].index, 1);
        assert_eq!(rows.row_records[0].row_count, 2);
    }

    #[test]
    fn review_mode_omits_staging_affordances() {
        let file = diff_file_with_path("src/main.rs");
        let theme = Theme::github_dark();

        let sidebar = render_file_entry(0, &file, 0, 80, false, theme);
        let header = render_file_header(&file, 80, false, theme);

        assert!(!line_text(&sidebar[0]).contains("[ ]"));
        assert!(!line_text(&header).contains("[unstaged]"));
    }

    fn diff_file_with_line(kind: DiffLineKind, content: &str) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions: usize::from(matches!(kind, DiffLineKind::Added)),
            deletions: usize::from(matches!(kind, DiffLineKind::Removed)),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![DiffLine {
                    kind,
                    old_line: (!matches!(kind, DiffLineKind::Added)).then_some(1),
                    new_line: (!matches!(kind, DiffLineKind::Removed)).then_some(1),
                    content: content.to_string(),
                }],
            }],
            binary: false,
        }
    }

    fn diff_file_with_path(path: &str) -> DiffFile {
        let mut file = diff_file_with_line(DiffLineKind::Context, "short");
        file.id = path.to_string();
        file.old_path = path.to_string();
        file.path = path.to_string();
        file.additions = 12;
        file.deletions = 3;
        file
    }

    fn line_prefix(line: &Line<'_>, width: usize) -> String {
        line_text(line).chars().take(width).collect()
    }

    fn line_suffix(line: &Line<'_>, width: usize) -> String {
        line_text(line).chars().skip(width).collect()
    }

    fn line_text(line: &Line<'_>) -> String {
        spans_text(&line.spans)
    }

    fn spans_text(spans: &[Span<'_>]) -> String {
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
