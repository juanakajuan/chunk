//! Ratatui pane layout and widget drawing for the application.
//!
//! Rendered terminal rows live in `rows`; this module owns layout, viewport
//! geometry updates, render-cache orchestration, and Ratatui widgets.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, FocusPane};
use crate::rows::{self, SidebarRowsInput};
use crate::theme::Theme;
use crate::viewport::RenderedDiffLines;

const SIDEBAR_WIDTH: u16 = 34;
const MIN_SPLIT_WIDTH: u16 = 100;
const PANE_BORDER_WIDTH: u16 = 2;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = active_theme();
    app.viewport.begin_frame();
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
    if !app.files_panel_visible {
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
    app.viewport.begin_sidebar(area, inner_height);
    app.ensure_scroll_bounds();

    let can_stage = app.can_stage();
    let row_counts = app
        .viewport
        .cached_sidebar_row_counts(content_width, can_stage, app.changeset.files.len(), || {
            rows::sidebar_row_counts(&app.changeset.files, content_width, can_stage, theme)
        })
        .to_vec();

    let rendered_rows = rows::sidebar_rows(SidebarRowsInput {
        files: &app.changeset.files,
        empty_message: app.empty_sidebar_message(),
        can_stage,
        selected_file_index: app.selected_file_index,
        sidebar_scroll: app.sidebar_scroll,
        row_counts: &row_counts,
        content_width,
        visible_height: inner_height,
        theme,
    });
    app.sidebar_scroll = rendered_rows.sidebar_scroll;
    app.viewport.begin_sidebar_rows();
    for record in rendered_rows.row_records {
        app.viewport
            .record_sidebar_rows(record.index, record.row_count);
    }

    let block = pane_block(" Files ", app.focus, FocusPane::Sidebar, theme);
    frame.render_widget(Paragraph::new(rendered_rows.lines).block(block), area);
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

    let title = format!(" {} ", rows::changeset_title(&app.changeset));
    let block = pane_block(title, app.focus, FocusPane::Diff, theme);

    let mut lines = rows::live_status_lines(app.live_error.as_deref(), content_width, theme);
    let visible_diff_height = inner_height.saturating_sub(lines.len());
    app.viewport.begin_diff(area, visible_diff_height);
    app.ensure_scroll_bounds();

    if visible_diff_height > 0 {
        lines.extend(render_selected_diff_lines(
            app,
            content_width,
            visible_diff_height,
            theme,
        ));
    }
    lines.truncate(inner_height);

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_keybind_bar(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(rows::keybind_bar_line(
            app.files_panel_visible,
            app.can_stage(),
            theme,
        ))
        .alignment(Alignment::Center),
        area,
    );
}

fn render_selected_diff_lines(
    app: &mut App,
    content_width: usize,
    visible_height: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    app.viewport
        .ensure_diff_lines_cache_len(app.changeset.files.len());

    let selected_file_index = app.selected_file_index;
    let can_stage = app.can_stage();
    if selected_file_index >= app.changeset.files.len() {
        return rows::no_diff_lines(app.no_diff_message(), content_width, theme);
    }

    let target_rows = app
        .diff_scroll
        .saturating_add(visible_height)
        .saturating_add(rows::DIFF_PREFETCH_ROWS);

    let needs_render = {
        let file = &app.changeset.files[selected_file_index];
        app.viewport.diff_lines_need_render(
            selected_file_index,
            file.id.as_str(),
            content_width,
            theme.syntax,
            can_stage,
            target_rows,
        )
    };

    if needs_render {
        app.ensure_selected_file_sources_loaded();
        let file = app.changeset.files[selected_file_index].clone();
        let rendered_rows =
            rows::diff_lines_until(&file, content_width, theme, can_stage, target_rows);
        app.viewport.cache_diff_lines(
            selected_file_index,
            RenderedDiffLines::new(
                file.id.clone(),
                content_width,
                theme.syntax,
                can_stage,
                rendered_rows.lines,
                rendered_rows.complete,
            ),
        );
    }

    app.ensure_scroll_bounds();

    app.viewport
        .visible_diff_lines(selected_file_index, app.diff_scroll, visible_height)
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
