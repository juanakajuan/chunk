//! Rendered terminal rows for sidebar, diff content, and status bars.
//!
//! This module owns row construction, wrapping, gutters, syntax advancement,
//! and display labels. `ui` owns pane layout and Ratatui widget drawing.

use std::ops::Range;
use std::str::Lines;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{
    Changeset, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStage, FileStatus, SourceSnapshot,
};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

const SIDEBAR_STAGE_GUTTER_WIDTH: usize = 8;
const SIDEBAR_REVIEW_GUTTER_WIDTH: usize = 4;
const DIFF_GUTTER_WIDTH: usize = 11;
pub(crate) const DIFF_PREFETCH_ROWS: usize = 120;
const RAIL_MARKER: &str = "▌";
const INTRALINE_MAX_BLOCK_LINES: usize = 32;
const INTRALINE_MAX_TOKENS: usize = 512;
const INTRALINE_MIN_WORD_SIMILARITY_PERCENT: usize = 35;
const INTRALINE_MIN_FALLBACK_SIMILARITY_PERCENT: usize = 50;

pub(crate) struct SidebarRowsInput<'a> {
    pub(crate) files: &'a [DiffFile],
    pub(crate) empty_message: &'static str,
    pub(crate) can_stage: bool,
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
            lines: vec![muted_line(input.empty_message, input.theme)],
            row_records: Vec::new(),
            sidebar_scroll: 0,
        };
    }

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
            input.can_stage,
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

pub(crate) fn hunk_offsets(
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
    can_stage: bool,
) -> Vec<usize> {
    if file.binary || file.hunks.is_empty() {
        return Vec::new();
    }

    let mut offset = wrap_line(
        render_file_header(file, content_width, can_stage, theme),
        content_width,
    )
    .len();
    let mut offsets = Vec::with_capacity(file.hunks.len());

    for hunk in &file.hunks {
        offsets.push(offset);
        offset += hunk_row_count(hunk, content_width, theme);
    }

    offsets
}

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
    hints.push("[n/N] hunk");
    hints.push("[e] edit");
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

    let mut push_diff_line = |line: &DiffLine, intraline_ranges: &[Range<usize>]| {
        lines.extend(diff_line(
            line,
            intraline_ranges,
            old_highlighter,
            new_highlighter,
            content_width,
            theme,
        ));
        lines.len() < target_rows
    };

    let mut line_index = 0;
    while line_index < hunk.lines.len() {
        let line = &hunk.lines[line_index];
        if !is_intraline_candidate(line.kind) {
            if !push_diff_line(line, &[]) {
                return false;
            }
            line_index += 1;
            continue;
        }

        let block_end = intraline_block_end(&hunk.lines, line_index);
        let block = &hunk.lines[line_index..block_end];
        let intraline_ranges = intraline_ranges_for_block(block);
        for (line, ranges) in block.iter().zip(&intraline_ranges) {
            if !push_diff_line(line, ranges) {
                return false;
            }
        }
        line_index = block_end;
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

fn hunk_row_count(hunk: &DiffHunk, content_width: usize, theme: Theme) -> usize {
    wrap_line(hunk_header_line(&hunk.header, theme), content_width).len()
        + hunk
            .lines
            .iter()
            .map(|line| diff_line_row_count(line, content_width))
            .sum::<usize>()
}

fn diff_line_row_count(line: &DiffLine, content_width: usize) -> usize {
    wrap_styled_spans(
        vec![Span::raw(expand_tabs(&line.content))],
        diff_content_width(content_width),
    )
    .len()
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
    intraline_ranges: &[Range<usize>],
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
        intraline_ranges,
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
    intraline_ranges: &[Range<usize>],
    old_highlighter: &mut DiffSyntaxHighlighter<'_>,
    new_highlighter: &mut DiffSyntaxHighlighter<'_>,
) -> Vec<Span<'static>> {
    match kind {
        DiffLineKind::Added => emphasize_spans(
            new_highlighter.highlight_line(content, content_style),
            intraline_ranges,
        ),
        DiffLineKind::Removed => emphasize_spans(
            old_highlighter.highlight_line(content, content_style),
            intraline_ranges,
        ),
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

fn intraline_ranges_for_block(block: &[DiffLine]) -> Vec<Vec<Range<usize>>> {
    let mut ranges = vec![Vec::new(); block.len()];
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for (index, line) in block.iter().enumerate() {
        match line.kind {
            DiffLineKind::Removed => removed.push((index, line)),
            DiffLineKind::Added => added.push((index, line)),
            DiffLineKind::Context | DiffLineKind::Meta => {}
        }
    }

    if removed.is_empty()
        || added.is_empty()
        || removed.len() > INTRALINE_MAX_BLOCK_LINES
        || added.len() > INTRALINE_MAX_BLOCK_LINES
    {
        return ranges;
    }

    let pairs = removed.into_iter().zip(added);
    for ((removed_index, removed_line), (added_index, added_line)) in pairs {
        let Some(pair_ranges) = intraline_pair_ranges(&removed_line.content, &added_line.content)
        else {
            continue;
        };

        ranges[removed_index] = pair_ranges.removed;
        ranges[added_index] = pair_ranges.added;
    }

    ranges
}

fn is_intraline_candidate(kind: DiffLineKind) -> bool {
    matches!(kind, DiffLineKind::Added | DiffLineKind::Removed)
}

fn intraline_block_end(lines: &[DiffLine], start: usize) -> usize {
    lines[start..]
        .iter()
        .position(|line| !is_intraline_candidate(line.kind))
        .map_or(lines.len(), |offset| start + offset)
}

struct IntralinePairRanges {
    removed: Vec<Range<usize>>,
    added: Vec<Range<usize>>,
}

fn intraline_pair_ranges(removed: &str, added: &str) -> Option<IntralinePairRanges> {
    let removed = expand_tabs(removed);
    let added = expand_tabs(added);
    if removed == added {
        return None;
    }

    let removed_tokens = intraline_tokens(&removed);
    let added_tokens = intraline_tokens(&added);
    if removed_tokens.is_empty()
        || added_tokens.is_empty()
        || removed_tokens.len() > INTRALINE_MAX_TOKENS
        || added_tokens.len() > INTRALINE_MAX_TOKENS
    {
        return None;
    }

    let common = common_intraline_tokens(&removed_tokens, &added_tokens);
    if !intraline_lines_are_related(
        &removed_tokens,
        &added_tokens,
        &common.removed,
        &common.added,
    ) {
        return None;
    }

    let removed = changed_intraline_ranges(&removed_tokens, &common.removed);
    let added = changed_intraline_ranges(&added_tokens, &common.added);
    if removed.is_empty() && added.is_empty() {
        return None;
    }

    Some(IntralinePairRanges { removed, added })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntralineTokenKind {
    Word,
    Whitespace,
    Punctuation,
}

#[derive(Debug, Clone, Copy)]
struct IntralineToken<'a> {
    text: &'a str,
    start: usize,
    end: usize,
    kind: IntralineTokenKind,
}

struct CommonIntralineTokens {
    removed: Vec<bool>,
    added: Vec<bool>,
}

fn intraline_tokens(text: &str) -> Vec<IntralineToken<'_>> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut char_start = 0;

    while let Some((byte_start, value)) = chars.next() {
        let kind = intraline_token_kind(value);
        let mut byte_end = byte_start + value.len_utf8();
        let mut char_end = char_start + 1;

        if kind != IntralineTokenKind::Punctuation {
            while let Some((next_byte, next_value)) = chars.peek().copied() {
                if intraline_token_kind(next_value) != kind {
                    break;
                }
                chars.next();
                byte_end = next_byte + next_value.len_utf8();
                char_end += 1;
            }
        }

        tokens.push(IntralineToken {
            text: &text[byte_start..byte_end],
            start: char_start,
            end: char_end,
            kind,
        });
        char_start = char_end;
    }

    tokens
}

fn intraline_token_kind(value: char) -> IntralineTokenKind {
    if value.is_alphanumeric() || value == '_' {
        IntralineTokenKind::Word
    } else if value.is_whitespace() {
        IntralineTokenKind::Whitespace
    } else {
        IntralineTokenKind::Punctuation
    }
}

fn common_intraline_tokens(
    removed: &[IntralineToken<'_>],
    added: &[IntralineToken<'_>],
) -> CommonIntralineTokens {
    let width = added.len() + 1;
    let mut lengths = vec![0; (removed.len() + 1) * width];

    for removed_index in (0..removed.len()).rev() {
        for added_index in (0..added.len()).rev() {
            let index = removed_index * width + added_index;
            lengths[index] = if intraline_tokens_equal(removed[removed_index], added[added_index]) {
                lengths[(removed_index + 1) * width + added_index + 1] + 1
            } else {
                lengths[(removed_index + 1) * width + added_index]
                    .max(lengths[removed_index * width + added_index + 1])
            };
        }
    }

    let mut removed_common = vec![false; removed.len()];
    let mut added_common = vec![false; added.len()];
    let mut removed_index = 0;
    let mut added_index = 0;
    while removed_index < removed.len() && added_index < added.len() {
        if intraline_tokens_equal(removed[removed_index], added[added_index]) {
            removed_common[removed_index] = true;
            added_common[added_index] = true;
            removed_index += 1;
            added_index += 1;
        } else if lengths[(removed_index + 1) * width + added_index]
            >= lengths[removed_index * width + added_index + 1]
        {
            removed_index += 1;
        } else {
            added_index += 1;
        }
    }

    CommonIntralineTokens {
        removed: removed_common,
        added: added_common,
    }
}

fn intraline_tokens_equal(left: IntralineToken<'_>, right: IntralineToken<'_>) -> bool {
    left.kind == right.kind && left.text == right.text
}

fn intraline_lines_are_related(
    removed: &[IntralineToken<'_>],
    added: &[IntralineToken<'_>],
    removed_common: &[bool],
    added_common: &[bool],
) -> bool {
    let removed_words = intraline_char_count(removed, None, is_word_token);
    let added_words = intraline_char_count(added, None, is_word_token);
    let word_denominator = removed_words.max(added_words);
    if word_denominator > 0 {
        let common_words = intraline_char_count(removed, Some(removed_common), is_word_token).min(
            intraline_char_count(added, Some(added_common), is_word_token),
        );
        return common_words * 100 >= word_denominator * INTRALINE_MIN_WORD_SIMILARITY_PERCENT;
    }

    let removed_non_whitespace = intraline_char_count(removed, None, is_non_whitespace_token);
    let added_non_whitespace = intraline_char_count(added, None, is_non_whitespace_token);
    let fallback_denominator = removed_non_whitespace.max(added_non_whitespace);
    if fallback_denominator == 0 {
        return false;
    }

    let common_non_whitespace =
        intraline_char_count(removed, Some(removed_common), is_non_whitespace_token).min(
            intraline_char_count(added, Some(added_common), is_non_whitespace_token),
        );
    common_non_whitespace * 100 >= fallback_denominator * INTRALINE_MIN_FALLBACK_SIMILARITY_PERCENT
}

fn intraline_char_count(
    tokens: &[IntralineToken<'_>],
    common: Option<&[bool]>,
    include_token: fn(IntralineTokenKind) -> bool,
) -> usize {
    tokens
        .iter()
        .enumerate()
        .filter(|(index, token)| {
            include_token(token.kind) && common.is_none_or(|common| common[*index])
        })
        .map(|(_, token)| intraline_token_len(token))
        .sum()
}

fn is_word_token(kind: IntralineTokenKind) -> bool {
    kind == IntralineTokenKind::Word
}

fn is_non_whitespace_token(kind: IntralineTokenKind) -> bool {
    kind != IntralineTokenKind::Whitespace
}

fn intraline_token_len(token: &IntralineToken<'_>) -> usize {
    token.end - token.start
}

fn changed_intraline_ranges(tokens: &[IntralineToken<'_>], common: &[bool]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut range_start = None;
    let mut range_end = 0;

    for (token, is_common) in tokens.iter().zip(common) {
        if *is_common {
            if let Some(start) = range_start.take() {
                ranges.push(start..range_end);
            }
            continue;
        }

        range_start.get_or_insert(token.start);
        range_end = token.end;
    }

    if let Some(start) = range_start {
        ranges.push(start..range_end);
    }

    ranges
}

fn emphasize_spans(spans: Vec<Span<'static>>, ranges: &[Range<usize>]) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return spans;
    }

    let mut chars = styled_chars(spans);
    for range in ranges {
        let start = range.start.min(chars.len());
        let end = range.end.min(chars.len());
        for character in &mut chars[start..end] {
            character.style = character.style.add_modifier(Modifier::BOLD);
        }
    }

    chars_to_spans(&chars)
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
    fn hunk_offsets_match_wrapped_rendered_rows() {
        let theme = Theme::github_dark();
        let file = diff_file_with_hunks(vec![
            (
                "@@ -1 +1 @@",
                vec![added_diff_line(
                    "alpha beta gamma delta epsilon zeta eta theta iota",
                )],
            ),
            ("@@ -10 +10 @@", vec![added_diff_line("short")]),
        ]);
        let content_width = DIFF_GUTTER_WIDTH + 12;

        let offsets = hunk_offsets(&file, content_width, theme, true);
        let lines = diff_lines_until(&file, content_width, theme, true, usize::MAX).lines;

        assert_eq!(offsets.len(), 2);
        assert_eq!(line_text(&lines[offsets[0]]), " @@ -1 +1 @@");
        assert_eq!(line_text(&lines[offsets[1]]), " @@ -10 +10 @@");
        assert!(offsets[1] > offsets[0] + 2);
    }

    #[test]
    fn intraline_highlights_inserted_spans() {
        let removed = r#"let title = format!("{} +{}", path, additions);"#;
        let added = r#"let title = format!("{} +{} -{}", path, additions, deletions);"#;
        let file = diff_file_with_lines(vec![removed_diff_line(removed), added_diff_line(added)]);
        let lines = diff_lines_until(
            &file,
            DIFF_GUTTER_WIDTH + 120,
            Theme::github_dark(),
            true,
            usize::MAX,
        )
        .lines;
        let added_line = rendered_line_with_text(&lines, added);

        assert_bold_span(added_line, " -{}");
        assert_bold_span(added_line, ", deletions");
    }

    #[test]
    fn intraline_highlights_replaced_spans_on_both_sides() {
        let removed = "let total = add(left, right);";
        let added = "let total = add(left, extra);";
        let file = diff_file_with_lines(vec![removed_diff_line(removed), added_diff_line(added)]);
        let lines = diff_lines_until(
            &file,
            DIFF_GUTTER_WIDTH + 80,
            Theme::github_dark(),
            true,
            usize::MAX,
        )
        .lines;

        assert_bold_span(rendered_line_with_text(&lines, removed), "right");
        assert_bold_span(rendered_line_with_text(&lines, added), "extra");
    }

    #[test]
    fn unrelated_changed_lines_skip_intraline_highlighting() {
        let removed = "fn alpha() -> bool { true }";
        let added = "struct Beta { value: usize }";
        let file = diff_file_with_lines(vec![removed_diff_line(removed), added_diff_line(added)]);
        let lines = diff_lines_until(
            &file,
            DIFF_GUTTER_WIDTH + 80,
            Theme::github_dark(),
            true,
            usize::MAX,
        )
        .lines;

        assert_no_bold_spans(rendered_line_with_text(&lines, removed));
        assert_no_bold_spans(rendered_line_with_text(&lines, added));
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
            empty_message: "No tracked changes",
            can_stage: true,
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
            empty_message: "No tracked changes",
            can_stage: true,
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

    fn diff_file_with_lines(lines: Vec<DiffLine>) -> DiffFile {
        let additions = lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .count();
        let deletions = lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Removed)
            .count();
        let old_lines = lines
            .iter()
            .filter(|line| !matches!(line.kind, DiffLineKind::Added | DiffLineKind::Meta))
            .count() as u32;
        let new_lines = lines
            .iter()
            .filter(|line| !matches!(line.kind, DiffLineKind::Removed | DiffLineKind::Meta))
            .count() as u32;

        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions,
            deletions,
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_lines,
                new_start: 1,
                new_lines,
                lines,
            }],
            binary: false,
        }
    }

    fn diff_file_with_hunks(hunks: Vec<(&str, Vec<DiffLine>)>) -> DiffFile {
        let additions = hunks
            .iter()
            .flat_map(|(_, lines)| lines)
            .filter(|line| line.kind == DiffLineKind::Added)
            .count();
        let deletions = hunks
            .iter()
            .flat_map(|(_, lines)| lines)
            .filter(|line| line.kind == DiffLineKind::Removed)
            .count();
        let hunks = hunks
            .into_iter()
            .enumerate()
            .map(|(index, (header, lines))| DiffHunk {
                header: header.to_string(),
                old_start: index as u32 + 1,
                old_lines: 1,
                new_start: index as u32 + 1,
                new_lines: 1,
                lines,
            })
            .collect();

        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: FileStage::Unstaged,
            additions,
            deletions,
            hunks,
            binary: false,
        }
    }

    fn removed_diff_line(content: &str) -> DiffLine {
        DiffLine {
            kind: DiffLineKind::Removed,
            old_line: Some(1),
            new_line: None,
            content: content.to_string(),
        }
    }

    fn added_diff_line(content: &str) -> DiffLine {
        DiffLine {
            kind: DiffLineKind::Added,
            old_line: None,
            new_line: Some(1),
            content: content.to_string(),
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

    fn rendered_line_with_text<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Line<'static> {
        lines
            .iter()
            .find(|line| line_text(line).contains(text))
            .expect("rendered diff line should exist")
    }

    fn assert_bold_span(line: &Line<'_>, text: &str) {
        assert!(
            line.spans.iter().any(|span| {
                span.content.as_ref() == text && span.style.add_modifier.contains(Modifier::BOLD)
            }),
            "expected bold span {text:?} in {:?}",
            line.spans
        );
    }

    fn assert_no_bold_spans(line: &Line<'_>) {
        assert!(
            !line
                .spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD)),
            "expected no bold spans in {:?}",
            line.spans
        );
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
