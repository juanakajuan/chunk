use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, FocusPane};
use crate::model::{DiffFile, DiffLineKind, FileStatus};
use crate::theme::Theme;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = Theme::matte_box();
    frame.render_widget(Block::default().style(theme.base_style()), frame.area());

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, root[0], app, theme);
    render_body(frame, root[1], app, theme);
    render_status(frame, root[2], app, theme);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    let line = Line::from(vec![
        Span::styled(
            " chunk ",
            Style::default().fg(theme.background).bg(theme.accent),
        ),
        Span::styled(
            format!(" {} ", app.changeset.source_label),
            Style::default().fg(theme.text).bg(theme.panel_alt),
        ),
        Span::styled(
            format!(" {} files", app.changeset.files.len()),
            Style::default().fg(theme.muted).bg(theme.panel_alt),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(line)
            .style(Style::default().fg(theme.text).bg(theme.panel_alt))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_body(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    if area.width >= MIN_SPLIT_WIDTH {
        let sidebar_width = SIDEBAR_WIDTH.min(area.width.saturating_sub(40));
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(sidebar_width),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);

        render_sidebar(frame, chunks[0], app, theme);
        render_divider(frame, chunks[1], theme);
        render_diff(frame, chunks[2], app, theme);
    } else {
        let sidebar_height = area.height.min(9);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(sidebar_height),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);

        render_sidebar(frame, chunks[0], app, theme);
        render_divider(frame, chunks[1], theme);
        render_diff(frame, chunks[2], app, theme);
    }
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    app.sidebar_view_height = inner_height;
    app.ensure_scroll_bounds();

    let border_color = if app.focus == FocusPane::Sidebar {
        theme.border_active
    } else {
        theme.border
    };
    let block = Block::default()
        .title(" Files ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().fg(theme.text).bg(theme.panel));

    let lines = if app.changeset.files.is_empty() {
        vec![Line::styled(
            "No tracked changes",
            Style::default().fg(theme.muted).bg(theme.panel),
        )]
    } else {
        app.changeset
            .files
            .iter()
            .enumerate()
            .skip(app.sidebar_scroll)
            .take(inner_height)
            .map(|(index, file)| render_file_entry(index, file, app.selected_file_index, theme))
            .collect()
    };

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_file_entry(
    index: usize,
    file: &DiffFile,
    selected_index: usize,
    theme: Theme,
) -> Line<'static> {
    let selected = index == selected_index;
    let base = if selected {
        Style::default().fg(theme.text).bg(theme.selected)
    } else {
        Style::default().fg(theme.text).bg(theme.panel)
    };
    let status_style = base.fg(status_color(file.status, theme));
    let prefix = if selected { ">" } else { " " };
    let stats = format!(" +{} -{}", file.additions, file.deletions);

    Line::from(vec![
        Span::styled(format!("{} ", prefix), base),
        Span::styled(file.status.marker().to_string(), status_style),
        Span::styled(" ", base),
        Span::styled(file.display_path().to_string(), base),
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
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    app.diff_view_height = inner_height;
    app.ensure_scroll_bounds();

    let title = app.selected_file().map_or(" Diff ".to_string(), |file| {
        format!(
            " {}  +{} -{} ",
            file.display_path(),
            file.additions,
            file.deletions
        )
    });
    let border_color = if app.focus == FocusPane::Diff {
        theme.border_active
    } else {
        theme.border
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().fg(theme.text).bg(theme.background));

    let lines = app.selected_file().map_or_else(
        || {
            vec![Line::styled(
                "No diff to review. Make a tracked change, then run chunk diff.",
                Style::default().fg(theme.muted).bg(theme.background),
            )]
        },
        |file| render_diff_lines(file, theme),
    );

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.diff_scroll as u16, 0)),
        area,
    );
}

fn render_diff_lines(file: &DiffFile, theme: Theme) -> Vec<Line<'static>> {
    if file.binary {
        return vec![Line::styled(
            "Binary file changed",
            Style::default().fg(theme.muted).bg(theme.background),
        )];
    }

    if file.hunks.is_empty() {
        return vec![Line::styled(
            "File changed without textual hunks",
            Style::default().fg(theme.muted).bg(theme.background),
        )];
    }

    let mut lines = Vec::new();
    for hunk in &file.hunks {
        lines.push(Line::styled(
            hunk.header.clone(),
            Style::default()
                .fg(theme.accent)
                .bg(theme.panel)
                .add_modifier(Modifier::BOLD),
        ));

        for line in &hunk.lines {
            let (marker, fg, bg) = match line.kind {
                DiffLineKind::Context => (" ", theme.text, theme.background),
                DiffLineKind::Added => ("+", theme.added, theme.added_bg),
                DiffLineKind::Removed => ("-", theme.removed, theme.removed_bg),
                DiffLineKind::Meta => (" ", theme.muted, theme.background),
            };
            let style = Style::default().fg(fg).bg(bg);
            let number_style = Style::default().fg(theme.muted).bg(bg);
            let old_line = format_line_number(line.old_line);
            let new_line = format_line_number(line.new_line);

            lines.push(Line::from(vec![
                Span::styled(format!("{} ", marker), style),
                Span::styled(old_line, number_style),
                Span::styled(" ", number_style),
                Span::styled(new_line, number_style),
                Span::styled("  ", number_style),
                Span::styled(line.content.clone(), style),
            ]));
        }
    }

    lines
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    let focus = match app.focus {
        FocusPane::Sidebar => "files",
        FocusPane::Diff => "diff",
    };
    let text = format!(
        " focus={}  tab switch  j/k move  arrows scroll  g/G top/bottom  q quit ",
        focus
    );

    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme.muted).bg(theme.panel_alt)),
        area,
    );
}

fn format_line_number(line: Option<u32>) -> String {
    line.map_or_else(|| "    ".to_string(), |line| format!("{line:>4}"))
}

fn status_color(status: FileStatus, theme: Theme) -> ratatui::style::Color {
    match status {
        FileStatus::Added => theme.added,
        FileStatus::Deleted => theme.removed,
        FileStatus::Modified => theme.accent,
        FileStatus::Renamed | FileStatus::Copied => theme.warning,
    }
}
