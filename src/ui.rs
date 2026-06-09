//! Ratatui pane layout and widget drawing for the application.
//!
//! Rendered terminal rows and render-cache orchestration live in `App`; this
//! module owns layout and Ratatui widgets.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, FocusPane};
use crate::theme::Theme;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;
const PANE_BORDER_WIDTH: u16 = 2;

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
}

fn render_keybind_bar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(app.keybind_bar_line(theme)).alignment(Alignment::Center),
        area,
    );
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
