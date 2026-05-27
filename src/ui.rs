use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, FocusPane, RenderedDiffLines};
use crate::model::{DiffFile, DiffHunk, DiffLineKind, FileStatus};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;
const PANE_BORDER_WIDTH: u16 = 2;
const SIDEBAR_GUTTER_WIDTH: usize = 4;
const DIFF_GUTTER_WIDTH: usize = 11;
const RAIL_MARKER: &str = "▌";
const NO_TRACKED_CHANGES: &str = "No tracked changes";
const NO_DIFF_MESSAGE: &str = "No diff to review. Make a tracked change, then run chunk diff.";

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = active_theme();
    app.sidebar_area = None;
    app.diff_area = None;
    frame.render_widget(Block::default().style(theme.base_style()), frame.area());
    render_body(frame, frame.area(), app, theme);
}

fn active_theme() -> Theme {
    match option_env!("CHUNK_THEME") {
        Some("github-dark") => Theme::github_dark(),
        _ => Theme::matte_box(),
    }
}

fn render_body(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let (direction, sidebar_size) = body_layout(area);
    let chunks = Layout::default()
        .direction(direction)
        .constraints([
            Constraint::Length(sidebar_size),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

    render_sidebar(frame, chunks[0], app, theme);
    render_divider(frame, chunks[1], theme);
    render_diff(frame, chunks[2], app, theme);
}

fn body_layout(area: Rect) -> (Direction, u16) {
    if area.width >= MIN_SPLIT_WIDTH {
        let sidebar_width = SIDEBAR_WIDTH.min(area.width.saturating_sub(40));
        return (Direction::Horizontal, sidebar_width);
    }

    (Direction::Vertical, area.height.min(9))
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    app.sidebar_area = Some(area);
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    app.sidebar_view_height = inner_height;
    app.ensure_scroll_bounds();

    let block = pane_block(" Files ", app.focus, FocusPane::Sidebar, theme);
    let lines = sidebar_lines(app, content_width, inner_height, theme);

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn sidebar_lines(
    app: &mut App,
    content_width: usize,
    visible_height: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    app.sidebar_row_indices.clear();

    if app.changeset.files.is_empty() {
        return vec![muted_line(NO_TRACKED_CHANGES, theme)];
    }

    ensure_wrapped_sidebar_selection_visible(app, content_width, visible_height, theme);

    let mut lines = Vec::new();
    for (index, file) in app
        .changeset
        .files
        .iter()
        .enumerate()
        .skip(app.sidebar_scroll)
    {
        for line in render_file_entry(index, file, app.selected_file_index, content_width, theme) {
            if lines.len() >= visible_height {
                return lines;
            }

            app.sidebar_row_indices.push(index);
            lines.push(line);
        }
    }

    lines
}

fn render_file_entry(
    index: usize,
    file: &DiffFile,
    selected_index: usize,
    content_width: usize,
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
    let used_width = SIDEBAR_GUTTER_WIDTH + display_width(&label) + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let rail = if selected { RAIL_MARKER } else { " " };

    let prefix = vec![
        Span::styled(rail, marker_style),
        Span::styled(" ", base),
        Span::styled(file.status.marker().to_string(), status_style),
        Span::styled(" ", base),
    ];
    let mut content = vec![Span::styled(label, base), Span::styled(padding, base)];
    push_stat_spans(&mut content, file, background, theme);

    if content_width <= SIDEBAR_GUTTER_WIDTH {
        let mut spans = prefix;
        spans.extend(content);
        return wrap_line(Line::from(spans), content_width);
    }

    wrap_sidebar_content(
        prefix,
        continuation_prefix(rail, marker_style, base),
        content,
        content_width,
    )
}

fn ensure_wrapped_sidebar_selection_visible(
    app: &mut App,
    content_width: usize,
    visible_height: usize,
    theme: Theme,
) {
    let file_count = app.changeset.files.len();
    if file_count == 0 {
        app.sidebar_scroll = 0;
        return;
    }

    let selected_index = app.selected_file_index.min(file_count - 1);
    app.sidebar_scroll = app.sidebar_scroll.min(file_count - 1);

    if selected_index < app.sidebar_scroll {
        app.sidebar_scroll = selected_index;
        return;
    }

    let row_counts = sidebar_row_counts(&app.changeset.files, content_width, theme);
    if !sidebar_selection_visible(
        &row_counts,
        app.sidebar_scroll,
        selected_index,
        visible_height,
    ) {
        app.sidebar_scroll =
            sidebar_scroll_for_selected(&row_counts, selected_index, visible_height);
    }
}

fn sidebar_row_counts(files: &[DiffFile], content_width: usize, theme: Theme) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| render_file_entry(index, file, usize::MAX, content_width, theme).len())
        .collect()
}

fn sidebar_selection_visible(
    row_counts: &[usize],
    scroll: usize,
    selected_index: usize,
    visible_height: usize,
) -> bool {
    if selected_index < scroll {
        return false;
    }

    let visible_height = visible_height.max(1);
    let rows_before_selected: usize = row_counts[scroll..selected_index].iter().sum();
    if rows_before_selected >= visible_height {
        return false;
    }

    let selected_rows = row_counts[selected_index];
    rows_before_selected == 0 || rows_before_selected + selected_rows <= visible_height
}

fn sidebar_scroll_for_selected(
    row_counts: &[usize],
    selected_index: usize,
    visible_height: usize,
) -> usize {
    let visible_height = visible_height.max(1);
    let mut scroll = selected_index;
    let mut rows = row_counts[selected_index];

    while scroll > 0 {
        let previous_rows = row_counts[scroll - 1];
        if rows + previous_rows > visible_height {
            break;
        }

        scroll -= 1;
        rows += previous_rows;
    }

    scroll
}

fn wrap_sidebar_content(
    first_prefix: Vec<Span<'static>>,
    continuation_prefix: Vec<Span<'static>>,
    content: Vec<Span<'static>>,
    content_width: usize,
) -> Vec<Line<'static>> {
    wrap_styled_spans(content, content_width.saturating_sub(SIDEBAR_GUTTER_WIDTH))
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

fn continuation_prefix(rail: &str, marker_style: Style, base: Style) -> Vec<Span<'static>> {
    vec![
        Span::styled(rail.to_string(), marker_style),
        Span::styled(" ".repeat(SIDEBAR_GUTTER_WIDTH - 1), base),
    ]
}

fn render_divider(frame: &mut Frame<'_>, area: Rect, theme: Theme) {
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(theme.border).bg(theme.background)),
        area,
    );
}

fn render_diff(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    app.diff_area = Some(area);
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    app.diff_view_height = inner_height;
    app.ensure_scroll_bounds();

    let title = format!(" {} ", changeset_title(app));
    let block = pane_block(title, app.focus, FocusPane::Diff, theme);

    let lines = render_selected_diff_lines(app, content_width, inner_height, theme);

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_diff_lines(file: &DiffFile, content_width: usize, theme: Theme) -> Vec<Line<'static>> {
    let mut lines = wrap_line(
        render_file_header(file, content_width, theme),
        content_width,
    );

    if file.binary {
        lines.extend(wrap_line(
            muted_line("Binary file changed", theme),
            content_width,
        ));
        return lines;
    }

    if file.hunks.is_empty() {
        lines.extend(wrap_line(
            muted_line("File changed without textual hunks", theme),
            content_width,
        ));
        return lines;
    }

    let mut old_highlighter = SyntaxHighlighter::for_path(diff_old_path(file), theme.syntax);
    let mut new_highlighter = SyntaxHighlighter::for_path(diff_new_path(file), theme.syntax);

    for hunk in &file.hunks {
        push_hunk_lines(
            &mut lines,
            hunk,
            &mut old_highlighter,
            &mut new_highlighter,
            content_width,
            theme,
        );
    }

    lines
}

fn push_hunk_lines(
    lines: &mut Vec<Line<'static>>,
    hunk: &DiffHunk,
    old_highlighter: &mut SyntaxHighlighter,
    new_highlighter: &mut SyntaxHighlighter,
    content_width: usize,
    theme: Theme,
) {
    lines.extend(wrap_line(
        hunk_header_line(&hunk.header, theme),
        content_width,
    ));

    for line in &hunk.lines {
        lines.extend(diff_line(
            line,
            old_highlighter,
            new_highlighter,
            content_width,
            theme,
        ));
    }
}

fn render_selected_diff_lines(
    app: &mut App,
    content_width: usize,
    visible_height: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let Some(file) = app.changeset.files.get(app.selected_file_index).cloned() else {
        return vec![muted_line(NO_DIFF_MESSAGE, theme)];
    };

    if diff_cache_needs_render(app.diff_lines_cache.as_ref(), &file, content_width, theme) {
        app.diff_lines_cache = Some(RenderedDiffLines {
            file_id: file.id.clone(),
            content_width,
            syntax_palette: theme.syntax,
            lines: render_diff_lines(&file, content_width, theme),
        });
    }

    app.ensure_scroll_bounds();

    match app.diff_lines_cache.as_ref() {
        Some(cache) => visible_diff_lines(&cache.lines, app.diff_scroll, visible_height),
        None => Vec::new(),
    }
}

fn diff_cache_needs_render(
    cache: Option<&RenderedDiffLines>,
    file: &DiffFile,
    content_width: usize,
    theme: Theme,
) -> bool {
    match cache {
        Some(cache) => {
            cache.file_id != file.id
                || cache.content_width != content_width
                || cache.syntax_palette != theme.syntax
        }
        None => true,
    }
}

fn visible_diff_lines(
    lines: &[Line<'static>],
    scroll: usize,
    visible_height: usize,
) -> Vec<Line<'static>> {
    lines
        .iter()
        .skip(scroll)
        .take(visible_height)
        .cloned()
        .collect()
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

fn pane_block(
    title: impl Into<String>,
    current_focus: FocusPane,
    target_focus: FocusPane,
    theme: Theme,
) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(focus_border_color(current_focus, target_focus, theme)))
        .style(color_style(theme.text, theme.background))
}

fn focus_border_color(current: FocusPane, target: FocusPane, theme: Theme) -> Color {
    if current == target {
        theme.border_active
    } else {
        theme.border
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
    line: &crate::model::DiffLine,
    old_highlighter: &mut SyntaxHighlighter,
    new_highlighter: &mut SyntaxHighlighter,
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
    old_highlighter: &mut SyntaxHighlighter,
    new_highlighter: &mut SyntaxHighlighter,
) -> Vec<Span<'static>> {
    let expanded_content = expand_tabs(content);

    match kind {
        DiffLineKind::Added => new_highlighter.highlight_line(&expanded_content, content_style),
        DiffLineKind::Removed => old_highlighter.highlight_line(&expanded_content, content_style),
        DiffLineKind::Context if new_highlighter.is_enabled() => {
            let spans = new_highlighter.highlight_line(&expanded_content, content_style);
            old_highlighter.advance_line(&expanded_content);
            spans
        }
        DiffLineKind::Context => {
            let spans = old_highlighter.highlight_line(&expanded_content, content_style);
            new_highlighter.advance_line(&expanded_content);
            spans
        }
        DiffLineKind::Meta => vec![Span::styled(expanded_content, content_style)],
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

fn changeset_title(app: &App) -> String {
    let additions: usize = app.changeset.files.iter().map(|file| file.additions).sum();
    let deletions: usize = app.changeset.files.iter().map(|file| file.deletions).sum();
    let title = if app.changeset.title.is_empty() {
        "changeset"
    } else {
        &app.changeset.title
    };

    format!("{}  +{}  -{}", title, additions, deletions)
}

fn render_file_header(file: &DiffFile, content_width: usize, theme: Theme) -> Line<'static> {
    let label = file_header_label(file);
    let suffix = file_status_suffix(file.status);
    let stats = format_file_stats(file);
    let stats_width = display_width(&stats);
    let used_width = display_width(&label) + display_width(suffix) + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let style = color_style(theme.text, theme.background);
    let muted_style = color_style(theme.muted, theme.background);

    let mut spans = vec![
        Span::styled(label, style),
        Span::styled(suffix.to_string(), muted_style),
        Span::styled(padding, style),
    ];
    push_stat_spans(&mut spans, file, theme.background, theme);
    Line::from(spans)
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
    use crate::model::{Changeset, DiffLine};

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
            let lines = render_diff_lines(&file, content_width, Theme::github_dark());
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
        let lines = render_diff_lines(&file, content_width, Theme::github_dark());
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
        let mut app = app_with_files(
            vec![diff_file_with_path(
                "src/components/extremely_long_file_name_component.rs",
            )],
            0,
        );
        let content_width = 16;
        let lines = sidebar_lines(&mut app, content_width, 8, Theme::github_dark());

        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.width() <= content_width));
        assert_eq!(app.sidebar_row_indices, vec![0; lines.len()]);
    }

    #[test]
    fn sidebar_scroll_accounts_for_wrapped_rows() {
        let mut app = app_with_files(
            vec![
                diff_file_with_path("first_extremely_long_file_name.rs"),
                diff_file_with_path("second_extremely_long_file_name.rs"),
            ],
            1,
        );
        let lines = sidebar_lines(&mut app, 14, 2, Theme::github_dark());

        assert_eq!(app.sidebar_scroll, 1);
        assert_eq!(lines.len(), 2);
        assert_eq!(app.sidebar_row_indices, vec![1, 1]);
    }

    fn diff_file_with_line(kind: DiffLineKind, content: &str) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: "sample.unknown".to_string(),
            path: "sample.unknown".to_string(),
            status: FileStatus::Modified,
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

    fn app_with_files(files: Vec<DiffFile>, selected_file_index: usize) -> App {
        App {
            changeset: Changeset {
                title: String::new(),
                source_label: String::new(),
                files,
            },
            selected_file_index,
            focus: FocusPane::Sidebar,
            diff_scroll: 0,
            sidebar_scroll: 0,
            diff_view_height: 1,
            sidebar_view_height: 1,
            sidebar_area: None,
            diff_area: None,
            sidebar_row_indices: Vec::new(),
            diff_lines_cache: None,
        }
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
