use std::io;
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::model::{Changeset, DiffFile};
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Sidebar,
    Diff,
}

#[derive(Debug)]
pub struct App {
    pub changeset: Changeset,
    pub selected_file_index: usize,
    pub focus: FocusPane,
    pub diff_scroll: usize,
    pub sidebar_scroll: usize,
    pub diff_view_height: usize,
    pub sidebar_view_height: usize,
    pub sidebar_area: Option<Rect>,
    pub diff_area: Option<Rect>,
}

impl App {
    fn new(changeset: Changeset) -> Self {
        Self {
            changeset,
            selected_file_index: 0,
            focus: FocusPane::Sidebar,
            diff_scroll: 0,
            sidebar_scroll: 0,
            diff_view_height: 1,
            sidebar_view_height: 1,
            sidebar_area: None,
            diff_area: None,
        }
    }

    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.changeset.files.get(self.selected_file_index)
    }

    pub fn selected_file_count(&self) -> usize {
        self.selected_file().map_or(0, DiffFile::line_count)
    }

    pub fn visible_file_count(&self) -> usize {
        self.changeset.files.len()
    }

    pub fn ensure_scroll_bounds(&mut self) {
        self.diff_scroll = self.diff_scroll.min(self.max_diff_scroll());

        let max_sidebar_scroll = self
            .visible_file_count()
            .saturating_sub(self.sidebar_view_height.max(1));
        self.sidebar_scroll = self.sidebar_scroll.min(max_sidebar_scroll);

        if self.selected_file_index < self.sidebar_scroll {
            self.sidebar_scroll = self.selected_file_index;
        }

        let last_visible_sidebar_index =
            self.sidebar_scroll + self.sidebar_view_height.saturating_sub(1);
        if self.selected_file_index > last_visible_sidebar_index {
            self.sidebar_scroll = self
                .selected_file_index
                .saturating_sub(self.sidebar_view_height.saturating_sub(1));
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Left => self.focus = FocusPane::Sidebar,
            KeyCode::Right | KeyCode::Enter => self.focus = FocusPane::Diff,
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::PageDown => self.scroll_diff_by(self.diff_view_height),
            KeyCode::PageUp => self.scroll_diff_up_by(self.diff_view_height),
            KeyCode::Home | KeyCode::Char('g') => self.diff_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll_diff_to_bottom(),
            _ => {}
        }

        self.ensure_scroll_bounds();
        true
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_left_click(mouse.column, mouse.row)
            }
            MouseEventKind::ScrollDown => self.handle_wheel_down(mouse.column, mouse.row),
            MouseEventKind::ScrollUp => self.handle_wheel_up(mouse.column, mouse.row),
            MouseEventKind::Moved => self.handle_hover(mouse.column, mouse.row),
            _ => {}
        }

        self.ensure_scroll_bounds();
    }

    fn handle_left_click(&mut self, column: u16, row: u16) {
        if let Some(index) = self.sidebar_index_at(column, row) {
            self.focus = FocusPane::Sidebar;
            self.select_file(index);
            return;
        }

        if self.is_diff_at(column, row) {
            self.focus = FocusPane::Diff;
        }
    }

    fn handle_hover(&mut self, column: u16, row: u16) {
        if self.is_sidebar_at(column, row) {
            self.focus = FocusPane::Sidebar;
        } else if self.is_diff_at(column, row) {
            self.focus = FocusPane::Diff;
        }
    }

    fn handle_wheel_down(&mut self, column: u16, row: u16) {
        if self.is_sidebar_at(column, row) {
            self.focus = FocusPane::Sidebar;
            self.select_next_file_by(3);
        } else {
            self.focus = FocusPane::Diff;
            self.scroll_diff_by(3);
        }
    }

    fn handle_wheel_up(&mut self, column: u16, row: u16) {
        if self.is_sidebar_at(column, row) {
            self.focus = FocusPane::Sidebar;
            self.select_previous_file_by(3);
        } else {
            self.focus = FocusPane::Diff;
            self.scroll_diff_up_by(3);
        }
    }

    fn sidebar_index_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.sidebar_area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }

        let row_offset = row.saturating_sub(area.y + 1) as usize;
        let index = self.sidebar_scroll + row_offset;
        (index < self.changeset.files.len()).then_some(index)
    }

    fn is_sidebar_at(&self, column: u16, row: u16) -> bool {
        self.sidebar_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    fn is_diff_at(&self, column: u16, row: u16) -> bool {
        self.diff_area
            .is_some_and(|area| rect_contains(area, column, row))
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Sidebar => FocusPane::Diff,
            FocusPane::Diff => FocusPane::Sidebar,
        };
    }

    fn move_down(&mut self) {
        match self.focus {
            FocusPane::Sidebar => self.select_next_file(),
            FocusPane::Diff => self.scroll_diff_by(1),
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            FocusPane::Sidebar => self.select_previous_file(),
            FocusPane::Diff => self.scroll_diff_up_by(1),
        }
    }

    fn select_next_file(&mut self) {
        self.select_next_file_by(1);
    }

    fn select_previous_file(&mut self) {
        self.select_previous_file_by(1);
    }

    fn select_next_file_by(&mut self, amount: usize) {
        let max_index = self.changeset.files.len().saturating_sub(1);
        self.select_file(
            self.selected_file_index
                .saturating_add(amount)
                .min(max_index),
        );
    }

    fn select_previous_file_by(&mut self, amount: usize) {
        self.select_file(self.selected_file_index.saturating_sub(amount));
    }

    fn select_file(&mut self, index: usize) {
        if self.changeset.files.is_empty() {
            return;
        }

        self.selected_file_index = index.min(self.changeset.files.len() - 1);
        self.diff_scroll = 0;
    }

    fn scroll_diff_by(&mut self, amount: usize) {
        self.diff_scroll = self.diff_scroll.saturating_add(amount);
    }

    fn scroll_diff_up_by(&mut self, amount: usize) {
        self.diff_scroll = self.diff_scroll.saturating_sub(amount);
    }

    fn scroll_diff_to_bottom(&mut self) {
        self.diff_scroll = self.max_diff_scroll();
    }

    fn max_diff_scroll(&self) -> usize {
        self.selected_file_count()
            .saturating_sub(self.diff_view_height.max(1))
    }
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn rect_inner_contains(area: Rect, column: u16, row: u16) -> bool {
    column > area.x
        && column < area.x.saturating_add(area.width).saturating_sub(1)
        && row > area.y
        && row < area.y.saturating_add(area.height).saturating_sub(1)
}

pub fn run(changeset: Changeset) -> Result<()> {
    let mut app = App::new(changeset);
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if !app.handle_key(key) => break,
                Event::Key(_) => {}
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => {}
            }
        }
    }

    Ok(())
}
