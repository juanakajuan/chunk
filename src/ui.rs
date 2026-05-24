use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, FocusPane};
use crate::model::{DiffFile, DiffLineKind, FileStatus};
use crate::theme::Theme;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;
const PANE_BORDER_WIDTH: u16 = 2;
const RAIL_MARKER: &str = "▌";

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = Theme::matte_box();
    app.sidebar_area = None;
    app.diff_area = None;
    frame.render_widget(Block::default().style(theme.base_style()), frame.area());
    render_body(frame, frame.area(), app, theme);
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

    let block = Block::default()
        .title(" Files ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(focus_border_color(app.focus, FocusPane::Sidebar, theme)))
        .style(Style::default().fg(theme.text).bg(theme.background));

    let lines = if app.changeset.files.is_empty() {
        vec![muted_line("No tracked changes", theme)]
    } else {
        app.changeset
            .files
            .iter()
            .enumerate()
            .skip(app.sidebar_scroll)
            .take(inner_height)
            .map(|(index, file)| {
                render_file_entry(index, file, app.selected_file_index, content_width, theme)
            })
            .collect()
    };

    frame.render_widget(Paragraph::new(lines).block(block), area);
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
    let base = Style::default().fg(theme.text).bg(background);
    let marker_style = if selected {
        Style::default().fg(theme.accent).bg(background)
    } else {
        base
    };
    let status_style = Style::default()
        .fg(status_color(file.status, theme))
        .bg(background);
    let label = sidebar_file_label(file);
    let stats = format_file_stats(file);
    let used_width = 4 + display_width(&label) + display_width(&stats);
    let padding = if stats.is_empty() {
        String::new()
    } else {
        " ".repeat(content_width.saturating_sub(used_width).max(1))
    };

    Line::from(vec![
        Span::styled(if selected { RAIL_MARKER } else { " " }, marker_style),
        Span::styled(" ", base),
        Span::styled(file.status.marker().to_string(), status_style),
        Span::styled(" ", base),
        Span::styled(label, base),
        Span::styled(padding, base),
        Span::styled(stats, base.fg(theme.muted)),
    ])
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
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(focus_border_color(app.focus, FocusPane::Diff, theme)))
        .style(Style::default().fg(theme.text).bg(theme.background));

    let lines = app.selected_file().map_or_else(
        || {
            vec![muted_line(
                "No diff to review. Make a tracked change, then run chunk diff.",
                theme,
            )]
        },
        |file| render_diff_lines(file, content_width, theme),
    );

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.diff_scroll as u16, 0)),
        area,
    );
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

    for hunk in &file.hunks {
        lines.push(hunk_header_line(&hunk.header, theme));

        for line in &hunk.lines {
            lines.push(diff_line(
                line.kind,
                line.old_line,
                line.new_line,
                &line.content,
                theme,
            ));
        }
    }

    lines
}

fn focus_border_color(current: FocusPane, target: FocusPane, theme: Theme) -> Color {
    if current == target {
        theme.border_active
    } else {
        theme.border
    }
}

fn muted_line(text: impl Into<String>, theme: Theme) -> Line<'static> {
    Line::styled(
        text.into(),
        Style::default().fg(theme.muted).bg(theme.background),
    )
}

fn hunk_header_line(header: &str, theme: Theme) -> Line<'static> {
    Line::styled(
        format!(" {header}"),
        Style::default()
            .fg(theme.muted)
            .bg(theme.background_alt)
            .add_modifier(Modifier::BOLD),
    )
}

fn diff_line(
    kind: DiffLineKind,
    old_line: Option<u32>,
    new_line: Option<u32>,
    content: &str,
    theme: Theme,
) -> Line<'static> {
    let style = diff_line_style(kind, theme);
    let number_style = Style::default()
        .fg(theme.line_number_fg)
        .bg(theme.line_number_bg);

    Line::from(vec![
        Span::styled(RAIL_MARKER, style.rail),
        Span::styled(format_line_number(old_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(format_line_number(new_line), number_style),
        Span::styled(" ", number_style),
        Span::styled(style.marker, style.content),
        Span::styled(" ", style.content),
        Span::styled(expand_tabs(content), style.content),
    ])
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
        content: Style::default().fg(text_color).bg(background),
        rail: Style::default().fg(rail_color).bg(background),
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
    let stats_width = file_stats_width(file);
    let used_width = display_width(&label) + display_width(suffix) + stats_width;
    let padding = if stats_width == 0 {
        String::new()
    } else {
        " ".repeat(content_width.saturating_sub(used_width).max(1))
    };
    let style = Style::default().fg(theme.text).bg(theme.background);
    let muted_style = Style::default().fg(theme.muted).bg(theme.background);

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
    if file.additions > 0 {
        spans.push(Span::styled(
            format!("+{}", file.additions),
            Style::default().fg(theme.added).bg(background),
        ));
    }

    if file.additions > 0 && file.deletions > 0 {
        spans.push(Span::styled(" ", Style::default().bg(background)));
    }

    if file.deletions > 0 {
        spans.push(Span::styled(
            format!("-{}", file.deletions),
            Style::default().fg(theme.removed).bg(background),
        ));
    }
}

fn file_header_label(file: &DiffFile) -> String {
    if matches!(file.status, FileStatus::Renamed | FileStatus::Copied) && file.old_path != file.path
    {
        format!("{} -> {}", file.old_path, file.path)
    } else {
        file.display_path().to_string()
    }
}

fn sidebar_file_label(file: &DiffFile) -> String {
    if matches!(file.status, FileStatus::Renamed | FileStatus::Copied) && file.old_path != file.path
    {
        format!("{} -> {}", basename(&file.old_path), basename(&file.path))
    } else {
        basename(file.display_path()).to_string()
    }
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

fn file_stats_width(file: &DiffFile) -> usize {
    display_width(&format_file_stats(file))
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "  ")
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}
