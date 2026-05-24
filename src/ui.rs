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
        Some("matte-box") => Theme::matte_box(),
        _ => Theme::github_dark(),
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
    app: &App,
    content_width: usize,
    visible_height: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    if app.changeset.files.is_empty() {
        return vec![muted_line(NO_TRACKED_CHANGES, theme)];
    }

    app.changeset
        .files
        .iter()
        .enumerate()
        .skip(app.sidebar_scroll)
        .take(visible_height)
        .map(|(index, file)| {
            render_file_entry(index, file, app.selected_file_index, content_width, theme)
        })
        .collect()
}

fn render_file_entry(
    index: usize,
    file: &DiffFile,
    selected_index: usize,
    content_width: usize,
    theme: Theme,
) -> Line<'static> {
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
    let used_width = 4 + display_width(&label) + stats_width;
    let padding = padding_before_stats(content_width, used_width, stats_width);
    let rail = if selected { RAIL_MARKER } else { " " };

    let mut spans = vec![
        Span::styled(rail, marker_style),
        Span::styled(" ", base),
        Span::styled(file.status.marker().to_string(), status_style),
        Span::styled(" ", base),
        Span::styled(label, base),
        Span::styled(padding, base),
    ];
    push_stat_spans(&mut spans, file, background, theme);
    Line::from(spans)
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
    let mut lines = vec![render_file_header(file, content_width, theme)];

    if file.binary {
        lines.push(muted_line("Binary file changed", theme));
        return lines;
    }

    if file.hunks.is_empty() {
        lines.push(muted_line("File changed without textual hunks", theme));
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
    theme: Theme,
) {
    lines.push(hunk_header_line(&hunk.header, theme));

    for line in &hunk.lines {
        lines.push(diff_line(
            line.kind,
            line.old_line,
            line.new_line,
            &line.content,
            old_highlighter,
            new_highlighter,
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
    kind: DiffLineKind,
    old_line: Option<u32>,
    new_line: Option<u32>,
    content: &str,
    old_highlighter: &mut SyntaxHighlighter,
    new_highlighter: &mut SyntaxHighlighter,
    theme: Theme,
) -> Line<'static> {
    let style = diff_line_style(kind, theme);
    let number_style = color_style(theme.line_number_fg, theme.line_number_bg);
    let mut spans = vec![
        Span::styled(RAIL_MARKER, style.rail),
        Span::styled(format_line_number(old_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(format_line_number(new_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(style.marker, style.content),
        Span::styled(" ", style.content),
    ];

    spans.extend(highlight_diff_content(
        kind,
        content,
        style.content,
        old_highlighter,
        new_highlighter,
    ));

    Line::from(spans)
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
    let mut stats = Vec::new();
    if file.additions > 0 {
        stats.push(format!("+{}", file.additions));
    }
    if file.deletions > 0 {
        stats.push(format!("-{}", file.deletions));
    }
    stats.join(" ")
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "  ")
}

fn display_width(text: &str) -> usize {
    text.chars().count()
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
