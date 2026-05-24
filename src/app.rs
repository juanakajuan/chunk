use std::io;
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

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
        let max_diff_scroll = self
            .selected_file_count()
            .saturating_sub(self.diff_view_height.max(1));
        self.diff_scroll = self.diff_scroll.min(max_diff_scroll);

        let max_sidebar_scroll = self
            .visible_file_count()
            .saturating_sub(self.sidebar_view_height.max(1));
        self.sidebar_scroll = self.sidebar_scroll.min(max_sidebar_scroll);

        if self.selected_file_index < self.sidebar_scroll {
            self.sidebar_scroll = self.selected_file_index;
        }

        let sidebar_bottom = self.sidebar_scroll + self.sidebar_view_height.saturating_sub(1);
        if self.selected_file_index > sidebar_bottom {
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
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_diff_by(self.diff_view_height),
            KeyCode::PageUp => self.scroll_diff_up_by(self.diff_view_height),
            KeyCode::Home | KeyCode::Char('g') => self.diff_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll_diff_to_bottom(),
            _ => {}
        }

        self.ensure_scroll_bounds();
        true
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
        let max_index = self.changeset.files.len().saturating_sub(1);
        self.selected_file_index = (self.selected_file_index + 1).min(max_index);
        self.diff_scroll = 0;
    }

    fn select_previous_file(&mut self) {
        self.selected_file_index = self.selected_file_index.saturating_sub(1);
        self.diff_scroll = 0;
    }

    fn scroll_diff_by(&mut self, amount: usize) {
        self.diff_scroll = self.diff_scroll.saturating_add(amount);
    }

    fn scroll_diff_up_by(&mut self, amount: usize) {
        self.diff_scroll = self.diff_scroll.saturating_sub(amount);
    }

    fn scroll_diff_to_bottom(&mut self) {
        self.diff_scroll = self
            .selected_file_count()
            .saturating_sub(self.diff_view_height.max(1));
    }
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
            let Event::Key(key) = event::read()? else {
                continue;
            };

            if !app.handle_key(key) {
                break;
            }
        }
    }

    Ok(())
}
