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
use crate::keybind::{BuiltinAction, KeybindMap};
use crate::theme::Theme;
use crate::viewport::DiffScrollbar;

const SIDEBAR_WIDTH: u16 = 44;
const MIN_SPLIT_WIDTH: u16 = 100;
const NARROW_SIDEBAR_HEIGHT: u16 = 16;
const PANE_BORDER_WIDTH: u16 = 2;
const HELP_OVERLAY_WIDTH: u16 = 78;
const HELP_OVERLAY_MAX_HEIGHT: u16 = 24;
const HELP_SCROLLBAR_WIDTH: u16 = 1;
const HARNESS_OVERLAY_WIDTH: u16 = 62;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneContentSize {
    width: usize,
    height: usize,
}

type SidebarRows = fn(&mut App, Rect, usize, usize, Theme) -> Vec<Line<'static>>;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = app.theme();
    app.begin_render_frame();
    frame.render_widget(Block::default().style(theme.base_style()), frame.area());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    render_body(frame, chunks[0], app, theme);
    render_keybind_bar(frame, chunks[1], app, theme);
    render_help_overlay(frame, frame.area(), app, theme);
    render_harness_picker_overlay(frame, frame.area(), app, theme);
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

    render_sidebar_stack(frame, chunks[0], app, theme);
    render_diff(frame, chunks[1], app, theme);
}

fn body_layout(area: Rect) -> (Direction, u16) {
    if area.width >= MIN_SPLIT_WIDTH {
        let sidebar_width = SIDEBAR_WIDTH.min(area.width.saturating_sub(40));
        return (Direction::Horizontal, sidebar_width);
    }

    (Direction::Vertical, area.height.min(NARROW_SIDEBAR_HEIGHT))
}

fn render_sidebar_stack(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(2, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
        ])
        .split(area);

    let panes: [(&str, FocusPane, SidebarRows); 3] = [
        (" Files ", FocusPane::Sidebar, App::sidebar_rows),
        (" Branches ", FocusPane::Branches, App::branch_panel_rows),
        (" Commits ", FocusPane::Commits, App::commit_panel_rows),
    ];
    for (pane_area, (title, focus, rows)) in chunks.iter().copied().zip(panes) {
        let content = pane_content_size(pane_area);
        let lines = rows(app, pane_area, content.width, content.height, theme);
        let block = pane_block(title, app.focus(), focus, theme);
        frame.render_widget(Paragraph::new(lines).block(block), pane_area);
    }
}

fn pane_content_size(area: Rect) -> PaneContentSize {
    PaneContentSize {
        width: usize::from(area.width.saturating_sub(PANE_BORDER_WIDTH)),
        height: usize::from(area.height.saturating_sub(PANE_BORDER_WIDTH).max(1)),
    }
}

fn render_diff(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let content = pane_content_size(area);
    let pane = app.diff_pane_rows(area, content.width, content.height, theme);
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

fn render_keybind_bar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let line = app.keybind_bar_line(theme);
    let mode_tag = app.keybind_mode_tag_line(theme);
    let tag_width = mode_tag
        .as_ref()
        .map(|line| saturating_u16(line.width().min(usize::from(area.width))));
    let key_area = keybind_content_area(area, line.width(), tag_width);
    let lines = app.selectable_lines(key_area, vec![line], 0, 1, theme);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(color_style(theme.muted, theme.background)),
        key_area,
    );
    if let (Some(mode_tag), Some(tag_width)) = (mode_tag, tag_width) {
        if tag_width == 0 {
            return;
        }
        let tag_area = Rect {
            width: tag_width,
            ..area
        };
        frame.render_widget(
            Paragraph::new(mode_tag)
                .alignment(Alignment::Left)
                .style(color_style(theme.muted, theme.background)),
            tag_area,
        );
    }
}

fn keybind_content_area(area: Rect, line_width: usize, tag_width: Option<u16>) -> Rect {
    let Some(tag_width) = tag_width else {
        return area;
    };
    let line_width = saturating_u16(line_width.min(usize::from(area.width)));
    let centered_start = area.width.saturating_sub(line_width) / 2;
    if centered_start > tag_width {
        return area;
    }

    let offset = tag_width.saturating_add(1).min(area.width);
    Rect {
        x: area.x.saturating_add(offset),
        width: area.width.saturating_sub(offset),
        ..area
    }
}

fn render_help_overlay(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    if !app.help_overlay_visible() {
        return;
    }

    let mut lines = app.help_overlay_lines(HelpOverlayLayout::content_width(area), theme);
    let mut layout = HelpOverlayLayout::new(area, lines.len());
    if layout.overflows {
        lines = app.help_overlay_lines(layout.scrollbar_content_width(), theme);
        layout = layout.with_reserved_scrollbar(lines.len());
    }
    app.clamp_help_overlay_scroll(lines.len(), layout.visible_content_height);
    let total_rows = lines.len();
    let scroll = app.help_overlay_scroll();
    let keybinds = app.keybinds();
    let area = centered_rect(area, layout.width, layout.height);
    let lines = app.selectable_lines(
        Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: saturating_u16(layout.content_width),
            height: saturating_u16(layout.visible_content_height),
        },
        lines,
        scroll,
        layout.visible_content_height,
        theme,
    );
    let block = modal_block(help_overlay_title(theme, layout.overflows, keybinds), theme);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((saturating_u16(scroll), 0))
            .block(block)
            .style(color_style(theme.text, theme.background_alt)),
        area,
    );
    if layout.overflows {
        render_help_scrollbar(
            frame,
            help_scrollbar_area(area),
            total_rows,
            layout.visible_content_height,
            scroll,
            theme,
        );
    }
}

fn help_overlay_title(theme: Theme, scrollable: bool, keybinds: KeybindMap) -> Line<'static> {
    let mut spans = vec![
        Span::styled("─ ", color_style(theme.border_active, theme.background_alt)),
        Span::styled(
            "Keymap",
            color_style(theme.accent, theme.background_alt).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {}/Esc/{} closes ",
                keybinds.display(BuiltinAction::Help),
                keybinds.display(BuiltinAction::Quit)
            ),
            color_style(theme.muted, theme.background_alt),
        ),
    ];
    if scrollable {
        spans.push(Span::styled(
            format!(
                " {}/{} scroll ",
                keybinds.display(BuiltinAction::MoveDown),
                keybinds.display(BuiltinAction::MoveUp)
            ),
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
    let thumb = scrollbar_thumb(usize::from(area.height), total_rows, visible_rows, scroll);
    render_scrollbar(frame, area, thumb, theme.background_alt, theme);
}

fn render_harness_picker_overlay(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    if !app.harness_picker_visible() {
        return;
    }

    let width = modal_width(area, HARNESS_OVERLAY_WIDTH);
    let content_width = usize::from(width.saturating_sub(PANE_BORDER_WIDTH));
    let lines = app.harness_picker_lines(content_width, theme);
    let height = modal_height(area, saturating_u16(lines.len()));
    let area = centered_rect(area, width, height);
    let block = modal_block(harness_picker_title(theme), theme);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(color_style(theme.text, theme.background_alt)),
        area,
    );
}

fn harness_picker_title(theme: Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled("─ ", color_style(theme.border_active, theme.background_alt)),
        Span::styled(
            "AI Harness",
            color_style(theme.accent, theme.background_alt).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", color_style(theme.border_active, theme.background_alt)),
    ])
}

fn modal_block(title: Line<'static>, theme: Theme) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.border_active)
                .bg(theme.background_alt),
        )
        .style(color_style(theme.text, theme.background_alt))
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
    let lines = (0..usize::from(area.height))
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
        x: area.x.saturating_add(area.width.saturating_sub(2)),
        y: area.y.saturating_add(1),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelpOverlayLayout {
    width: u16,
    height: u16,
    content_width: usize,
    visible_content_height: usize,
    overflows: bool,
}

impl HelpOverlayLayout {
    fn content_width(area: Rect) -> usize {
        usize::from(help_overlay_width(area).saturating_sub(PANE_BORDER_WIDTH))
    }

    fn new(area: Rect, line_count: usize) -> Self {
        let width = help_overlay_width(area);
        let height = help_overlay_height(area, saturating_u16(line_count));
        let visible_content_height = usize::from(height.saturating_sub(PANE_BORDER_WIDTH));

        Self {
            width,
            height,
            content_width: usize::from(width.saturating_sub(PANE_BORDER_WIDTH)),
            visible_content_height,
            overflows: line_count > visible_content_height,
        }
    }

    fn scrollbar_content_width(self) -> usize {
        usize::from(
            self.width
                .saturating_sub(PANE_BORDER_WIDTH + HELP_SCROLLBAR_WIDTH),
        )
    }

    fn with_reserved_scrollbar(mut self, line_count: usize) -> Self {
        self.content_width = self.scrollbar_content_width();
        self.overflows = line_count > self.visible_content_height;
        self
    }
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn help_overlay_width(area: Rect) -> u16 {
    modal_width(area, HELP_OVERLAY_WIDTH)
}

fn help_overlay_height(area: Rect, content_height: u16) -> u16 {
    modal_height(area, content_height).min(HELP_OVERLAY_MAX_HEIGHT)
}

fn modal_width(area: Rect, preferred_width: u16) -> u16 {
    if area.width <= 4 {
        return area.width;
    }

    preferred_width.min(area.width - 4)
}

fn modal_height(area: Rect, content_height: u16) -> u16 {
    if area.height <= 2 {
        return area.height;
    }

    content_height
        .saturating_add(PANE_BORDER_WIDTH)
        .min(area.height - 2)
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let horizontal_margin = area.width.saturating_sub(width) / 2;
    let vertical_margin = area.height.saturating_sub(height) / 2;

    Rect {
        x: area.x.saturating_add(horizontal_margin),
        y: area.y.saturating_add(vertical_margin),
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
    styled_pane_block(title, current_focus == target_focus, theme)
}

fn styled_pane_block(title: impl Into<String>, focused: bool, theme: Theme) -> Block<'static> {
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
    use crate::model::{Changeset, CommitSummary, LocalBranch, RepositoryOverview};
    use crate::review_source::LoadedReview;
    use crate::theme::ThemeName;

    #[test]
    fn help_overlay_layout_preserves_first_pass_height_when_reserving_scrollbar() {
        let area = Rect::new(0, 0, 100, 30);
        let initial = HelpOverlayLayout::new(area, 30);

        assert_eq!(initial.width, HELP_OVERLAY_WIDTH);
        assert_eq!(initial.height, HELP_OVERLAY_MAX_HEIGHT);
        assert_eq!(initial.content_width, 76);
        assert_eq!(initial.visible_content_height, 22);
        assert!(initial.overflows);

        let remeasured = initial.with_reserved_scrollbar(22);

        assert_eq!(remeasured.height, initial.height);
        assert_eq!(remeasured.content_width, 75);
        assert!(!remeasured.overflows);
    }

    #[test]
    fn help_overlay_draws_custom_commands_from_config() {
        let mut app = app_with_commands(vec![custom_command("P", "publish", "git push")]);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
            .unwrap();

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = buffer_text(terminal.backend().buffer());

        assert!(
            buffer.contains("P  publish  git push"),
            "buffer was {buffer}"
        );
    }

    #[test]
    fn keybind_bar_renders_accent_filled_key_tokens() {
        let mut app = app_with_config(AppConfig {
            theme: ThemeName::GithubDark,
            commands: Vec::new(),
            ..AppConfig::default()
        });
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let theme = Theme::github_dark();
        let buffer = terminal.backend().buffer();
        let row = buffer.area.height.saturating_sub(1);
        let footer_debug = (0..buffer.area.width)
            .filter_map(|column| {
                let cell = buffer.cell((column, row))?;
                (cell.symbol() != " ").then(|| {
                    format!(
                        "{column}:{} fg={:?} bg={:?}",
                        cell.symbol(),
                        cell.fg,
                        cell.bg
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let has_accent_key = (0..buffer.area.width).any(|column| {
            let Some(cell) = buffer.cell((column, row)) else {
                return false;
            };

            cell.symbol() == "f" && cell.fg == theme.on_accent && cell.bg == theme.accent
        });

        assert!(
            has_accent_key,
            "footer f key should render with accent fill:\n{footer_debug}"
        );
    }

    #[test]
    fn sidebar_stack_draws_branches_and_commits_below_files() {
        let mut app = App::with_config(
            LoadedReview::worktree_with_repository_overview(
                Changeset {
                    title: String::new(),
                    source_label: String::new(),
                    files: Vec::new(),
                },
                RepositoryOverview {
                    branches: vec![LocalBranch {
                        name: "main".to_string(),
                        current: true,
                    }],
                    commits: vec![CommitSummary {
                        short_id: "abc1234".to_string(),
                        subject: "Add repository panels".to_string(),
                    }],
                },
            ),
            AppConfig::default(),
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = buffer_text(terminal.backend().buffer());

        assert!(buffer.contains("Files"), "buffer was {buffer}");
        assert!(buffer.contains("Branches"), "buffer was {buffer}");
        assert!(buffer.contains("* main"), "buffer was {buffer}");
        assert!(buffer.contains("Commits"), "buffer was {buffer}");
        assert!(
            buffer.contains("abc1234 Add repository panels"),
            "buffer was {buffer}"
        );
    }

    fn app_with_commands(commands: Vec<CustomCommandBinding>) -> App {
        app_with_config(AppConfig {
            commands,
            ..AppConfig::default()
        })
    }

    fn app_with_config(config: AppConfig) -> App {
        App::with_config(
            LoadedReview::worktree(Changeset {
                title: String::new(),
                source_label: String::new(),
                files: Vec::new(),
            }),
            config,
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
