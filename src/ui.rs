//! Ratatui pane layout and widget drawing for the application.
//!
//! Rendered terminal rows and render-cache orchestration live in `App`; this
//! module owns layout and Ratatui widgets.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, FocusPane};
use crate::theme::Theme;
use crate::viewport::DiffScrollbar;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;
const PANE_BORDER_WIDTH: u16 = 2;
const HELP_OVERLAY_WIDTH: u16 = 78;
const HELP_OVERLAY_MAX_HEIGHT: u16 = 24;

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
        _ => Theme::matte_box(),
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
    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    let lines = app.sidebar_rows(area, content_width, inner_height, theme);
    let block = pane_block(" Files ", app.focus(), FocusPane::Sidebar, theme);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_divider(frame: &mut Frame<'_>, area: Rect, theme: Theme) {
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(theme.border).bg(theme.background)),
        area,
    );
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
    let area = scrollbar.area();
    let track_style = color_style(theme.muted, theme.background);
    let thumb_style = color_style(theme.accent, theme.background);
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

fn render_keybind_bar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(app.keybind_bar_line(theme)).alignment(Alignment::Center),
        area,
    );
}

fn render_help_overlay(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    if !app.help_overlay_visible() {
        return;
    }

    let width = help_overlay_width(area);
    let content_width = width.saturating_sub(PANE_BORDER_WIDTH) as usize;
    let lines = app.help_overlay_lines(content_width, theme);
    let height = help_overlay_height(area, lines.len() as u16);
    let area = centered_rect(area, width, height);
    let block = Block::default()
        .title(" Keymap (?/Esc/q closes) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_active))
        .style(color_style(theme.text, theme.background_alt));

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(color_style(theme.text, theme.background_alt)),
        area,
    );
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
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(focus_border_color(current_focus, target_focus, theme)))
        .style(color_style(theme.text, theme.background))
}

fn focus_border_color(current_focus: FocusPane, target_focus: FocusPane, theme: Theme) -> Color {
    if current_focus == target_focus {
        theme.border_active
    } else {
        theme.border
    }
}

fn color_style(foreground: Color, background: Color) -> Style {
    Style::default().fg(foreground).bg(background)
}
