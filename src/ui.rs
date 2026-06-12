//! Ratatui pane layout and widget drawing for the application.
//!
//! Rendered terminal rows and render-cache orchestration live in `App`; this
//! module owns layout and Ratatui widgets.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, FocusPane};
use crate::theme::Theme;
use crate::viewport::DiffScrollbar;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;
const PANE_BORDER_WIDTH: u16 = 2;
const HELP_OVERLAY_WIDTH: u16 = 78;
const HELP_OVERLAY_MAX_HEIGHT: u16 = 24;
const HELP_SCROLLBAR_WIDTH: u16 = 1;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = active_theme();
    app.begin_render_frame();
    frame.render_widget(Block::default().style(theme.base_style()), frame.area());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    render_body(frame, chunks[0], app, theme);
    render_keybind_bar(frame, chunks[1], app, theme);
    render_help_overlay(frame, frame.area(), app, theme);
}

fn active_theme() -> Theme {
    match option_env!("CHUNK_THEME") {
        Some("github-dark") => Theme::github_dark(),
        _ => Theme::gruvbox_dark_hard(),
    }
}

fn render_body(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    if !app.files_panel_visible() {
        render_diff(frame, area, app, theme);
        return;
    }

    let (direction, sidebar_size) = body_layout(area);
    let chunks = Layout::default()
        .direction(direction)
        .constraints([Constraint::Length(sidebar_size), Constraint::Min(0)])
        .split(area);

    render_sidebar(frame, chunks[0], app, theme);
    render_diff(frame, chunks[1], app, theme);
}

fn body_layout(area: Rect) -> (Direction, u16) {
    if area.width >= MIN_SPLIT_WIDTH {
        let sidebar_width = SIDEBAR_WIDTH.min(area.width.saturating_sub(40));
        return (Direction::Horizontal, sidebar_width);
    }

    (Direction::Vertical, area.height.min(9))
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    let lines = app.sidebar_rows(area, content_width, inner_height, theme);
    let block = pane_block(" Files ", app.focus(), FocusPane::Sidebar, theme);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_diff(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    let pane = app.diff_pane_rows(area, content_width, inner_height, theme);
    let block = pane_block(pane.title, app.focus(), FocusPane::Diff, theme);
    frame.render_widget(Paragraph::new(pane.lines).block(block), area);
    if let Some(scrollbar) = pane.scrollbar {
        render_diff_scrollbar(frame, scrollbar, theme);
    }
}

fn render_diff_scrollbar(frame: &mut Frame<'_>, scrollbar: DiffScrollbar, theme: Theme) {
    let thumb = scrollbar.thumb();
    render_scrollbar(
        frame,
        scrollbar.area(),
        ScrollbarThumb {
            start: thumb.start,
            len: thumb.len,
        },
        theme.background,
        theme,
    );
}

fn render_keybind_bar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(app.keybind_bar_line(theme))
            .alignment(Alignment::Left)
            .style(color_style(theme.muted, theme.background)),
        area,
    );
}

fn render_help_overlay(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    if !app.help_overlay_visible() {
        return;
    }

    let width = help_overlay_width(area);
    let mut content_width = width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    let mut lines = app.help_overlay_lines(content_width, theme);
    let height = help_overlay_height(area, lines.len() as u16);
    let visible_content_height = height.saturating_sub(PANE_BORDER_WIDTH) as usize;
    let mut overflow = help_overlay_overflows(lines.len(), visible_content_height);
    if overflow {
        content_width = width.saturating_sub(PANE_BORDER_WIDTH + HELP_SCROLLBAR_WIDTH) as usize;
        lines = app.help_overlay_lines(content_width, theme);
        overflow = help_overlay_overflows(lines.len(), visible_content_height);
    }
    app.clamp_help_overlay_scroll(lines.len(), visible_content_height);
    let total_rows = lines.len();
    let scroll = app.help_overlay_scroll();
    let area = centered_rect(area, width, height);
    let block = Block::default()
        .title(help_overlay_title(theme, overflow))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.border_active)
                .bg(theme.background_alt),
        )
        .style(color_style(theme.text, theme.background_alt));

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((saturating_u16(scroll), 0))
            .block(block)
            .style(color_style(theme.text, theme.background_alt)),
        area,
    );
    if overflow {
        render_help_scrollbar(
            frame,
            help_scrollbar_area(area),
            total_rows,
            visible_content_height,
            scroll,
            theme,
        );
    }
}

fn help_overlay_title(theme: Theme, scrollable: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled("─ ", color_style(theme.border_active, theme.background_alt)),
        Span::styled(
            "Keymap",
            color_style(theme.accent, theme.background_alt).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  ?/Esc/q closes ",
            color_style(theme.muted, theme.background_alt),
        ),
    ];
    if scrollable {
        spans.push(Span::styled(
            " j/k scroll ",
            color_style(theme.muted, theme.background_alt),
        ));
    }

    Line::from(spans)
}

fn render_help_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    total_rows: usize,
    visible_rows: usize,
    scroll: usize,
    theme: Theme,
) {
    let thumb = scrollbar_thumb(area.height as usize, total_rows, visible_rows, scroll);
    render_scrollbar(frame, area, thumb, theme.background_alt, theme);
}

fn render_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    thumb: ScrollbarThumb,
    background: Color,
    theme: Theme,
) {
    let track_style = color_style(theme.muted, background);
    let thumb_style = color_style(theme.accent, background);
    let lines = (0..area.height as usize)
        .map(|row| {
            let in_thumb = row >= thumb.start && row < thumb.start.saturating_add(thumb.len);
            let (symbol, style) = if in_thumb {
                ("█", thumb_style)
            } else {
                ("│", track_style)
            };
            Line::from(Span::styled(symbol, style))
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(lines), area);
}

fn help_scrollbar_area(area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(2),
        y: area.y + 1,
        width: HELP_SCROLLBAR_WIDTH,
        height: area.height.saturating_sub(PANE_BORDER_WIDTH),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScrollbarThumb {
    start: usize,
    len: usize,
}

fn scrollbar_thumb(
    track_height: usize,
    total_rows: usize,
    visible_rows: usize,
    scroll: usize,
) -> ScrollbarThumb {
    let track_height = track_height.max(1);
    let max_thumb_len = track_height.saturating_sub(1).max(1);
    let thumb_len = ratio_ceil(visible_rows, track_height, total_rows)
        .max(1)
        .min(max_thumb_len);
    let track_range = track_height.saturating_sub(thumb_len);
    let scroll_range = total_rows.saturating_sub(visible_rows);
    let start = if track_range == 0 || scroll_range == 0 {
        0
    } else {
        ratio_round(scroll.min(scroll_range), track_range, scroll_range)
    };

    ScrollbarThumb {
        start,
        len: thumb_len,
    }
}

fn ratio_ceil(value: usize, numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return 0;
    }

    value
        .saturating_mul(numerator)
        .saturating_add(denominator.saturating_sub(1))
        / denominator
}

fn ratio_round(value: usize, numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return 0;
    }

    value
        .saturating_mul(numerator)
        .saturating_add(denominator / 2)
        / denominator
}

fn help_overlay_overflows(line_count: usize, visible_height: usize) -> bool {
    line_count > visible_height
}

fn saturating_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn help_overlay_width(area: Rect) -> u16 {
    if area.width <= 4 {
        return area.width;
    }

    HELP_OVERLAY_WIDTH.min(area.width - 4)
}

fn help_overlay_height(area: Rect, content_height: u16) -> u16 {
    if area.height <= 2 {
        return area.height;
    }

    content_height
        .saturating_add(PANE_BORDER_WIDTH)
        .min(HELP_OVERLAY_MAX_HEIGHT)
        .min(area.height - 2)
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let horizontal_margin = area.width.saturating_sub(width) / 2;
    let vertical_margin = area.height.saturating_sub(height) / 2;

    Rect {
        x: area.x + horizontal_margin,
        y: area.y + vertical_margin,
        width,
        height,
    }
}

fn pane_block(
    title: impl Into<String>,
    current_focus: FocusPane,
    target_focus: FocusPane,
    theme: Theme,
) -> Block<'static> {
    let focused = current_focus == target_focus;
    let border_color = if focused {
        theme.border_active
    } else {
        theme.border
    };
    Block::default()
        .title(pane_title(title.into(), focused, theme))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color).bg(theme.background))
        .style(color_style(theme.text, theme.background))
}

fn pane_title(title: String, focused: bool, theme: Theme) -> Line<'static> {
    let marker_color = if focused {
        theme.border_active
    } else {
        theme.border
    };
    let title_color = if focused { theme.accent } else { theme.muted };
    let mut title_style = color_style(title_color, theme.background);
    if focused {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }

    Line::from(vec![
        Span::styled("─ ", color_style(marker_color, theme.background)),
        Span::styled(title.trim().to_string(), title_style),
        Span::styled(" ", color_style(marker_color, theme.background)),
    ])
}

fn color_style(foreground: Color, background: Color) -> Style {
    Style::default().fg(foreground).bg(background)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use crate::config::AppConfig;
    use crate::custom_command::{CommandKey, CustomCommandBinding};
    use crate::model::Changeset;
    use crate::review_source::LoadedReview;

    #[test]
    fn help_overlay_draws_custom_commands_from_config() {
        let mut app = app_with_commands(vec![custom_command("P", "publish", "git push")]);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = buffer_text(terminal.backend().buffer());

        assert!(
            buffer.contains("P publish  git push"),
            "buffer was {buffer}"
        );
    }

    #[test]
    fn help_overlay_scrolls_long_keymaps() {
        let mut app = app_with_commands(vec![
            custom_command("A", "command 1", "true"),
            custom_command("B", "command 2", "true"),
            custom_command("C", "command 3", "true"),
            custom_command("D", "command 4", "true"),
            custom_command("E", "command 5", "true"),
            custom_command("F", "command 6", "true"),
        ]);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE))
            .unwrap();

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = buffer_text(terminal.backend().buffer());

        assert!(buffer.contains("Worktree-only"), "buffer was {buffer}");
        assert!(buffer.contains("█"), "buffer was {buffer}");
    }

    fn app_with_commands(commands: Vec<CustomCommandBinding>) -> App {
        App::with_config(
            LoadedReview::worktree(Changeset {
                title: String::new(),
                source_label: String::new(),
                files: Vec::new(),
            }),
            AppConfig { commands },
        )
    }

    fn custom_command(key: &str, label: &str, command: &str) -> CustomCommandBinding {
        CustomCommandBinding::new(
            CommandKey::parse(key).unwrap(),
            label.to_string(),
            command.to_string(),
        )
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let mut text = String::new();
        for cell in &buffer.content {
            text.push_str(cell.symbol());
        }
        text
    }
}
