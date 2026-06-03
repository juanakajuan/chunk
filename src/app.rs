//! Terminal application state and event loop.
//!
//! `App` owns selection, focus, scroll positions, live reload errors, and the
//! rendered diff cache. Rendering is delegated to `ui`; Git mutations are
//! delegated to `git`.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::git::{
    load_source_snapshots, load_worktree_diff, toggle_staging_for_file, worktree_root,
};
use crate::model::{Changeset, DiffFile};
use crate::theme::SyntaxPalette;
use crate::ui;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WORKTREE_RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);
const MOUSE_WHEEL_STEP: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Sidebar,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelDirection {
    Down,
    Up,
}

#[derive(Debug)]
pub struct App {
    /// Current diff data being reviewed.
    pub changeset: Changeset,
    /// Last live reload/watch error, rendered above the diff when present.
    pub live_error: Option<String>,
    /// Index into `changeset.files`.
    pub selected_file_index: usize,
    /// Pane receiving keyboard and mouse wheel actions.
    pub focus: FocusPane,
    /// First rendered diff row visible in the diff pane.
    pub diff_scroll: usize,
    /// First file index considered for sidebar rendering.
    pub sidebar_scroll: usize,
    /// Current diff viewport height, updated by the renderer.
    pub diff_view_height: usize,
    /// Current sidebar viewport height, updated by the renderer.
    pub sidebar_view_height: usize,
    /// Last sidebar rectangle, used to map mouse events.
    pub sidebar_area: Option<Rect>,
    /// Last diff rectangle, used to map mouse events.
    pub diff_area: Option<Rect>,
    /// Rendered sidebar row to file index mapping for click handling.
    pub sidebar_row_indices: Vec<usize>,
    /// Cached wrapped and highlighted diff lines for the selected file.
    pub diff_lines_cache: Option<RenderedDiffLines>,
}

#[derive(Debug, Clone)]
pub struct RenderedDiffLines {
    /// `DiffFile::id` for the cached file.
    pub file_id: String,
    /// Width used to wrap cached lines.
    pub content_width: usize,
    /// Syntax palette used while highlighting cached lines.
    pub syntax_palette: SyntaxPalette,
    /// Fully rendered, wrapped lines for the selected file.
    pub lines: Vec<Line<'static>>,
}

impl App {
    fn new(changeset: Changeset) -> Self {
        Self {
            changeset,
            live_error: None,
            selected_file_index: 0,
            focus: FocusPane::Sidebar,
            diff_scroll: 0,
            sidebar_scroll: 0,
            diff_view_height: 1,
            sidebar_view_height: 1,
            sidebar_area: None,
            diff_area: None,
            sidebar_row_indices: Vec::new(),
            diff_lines_cache: None,
        }
    }

    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.changeset.files.get(self.selected_file_index)
    }

    pub fn ensure_selected_file_sources_loaded(&mut self) {
        let source = &self.changeset.source;
        if let Some(file) = self.changeset.files.get_mut(self.selected_file_index) {
            load_source_snapshots(file, source);
        }
    }

    fn selected_file_line_count(&self) -> usize {
        let Some(file) = self.selected_file() else {
            return 0;
        };

        match self.diff_lines_cache.as_ref() {
            Some(cache) if cache.file_id.as_str() == file.id.as_str() => cache.lines.len(),
            _ => file.line_count(),
        }
    }

    fn file_count(&self) -> usize {
        self.changeset.files.len()
    }

    pub fn ensure_scroll_bounds(&mut self) {
        self.diff_scroll = self.diff_scroll.min(self.max_diff_scroll());
        self.sidebar_scroll = self.sidebar_scroll.min(self.max_sidebar_scroll());
        self.keep_selected_file_visible();
    }

    fn keep_selected_file_visible(&mut self) {
        if self.selected_file_index < self.sidebar_scroll {
            self.sidebar_scroll = self.selected_file_index;
            return;
        }

        let last_visible_sidebar_index =
            self.sidebar_scroll + self.sidebar_view_height.saturating_sub(1);
        if self.selected_file_index > last_visible_sidebar_index {
            self.sidebar_scroll = self
                .selected_file_index
                .saturating_sub(self.sidebar_view_height.saturating_sub(1));
        }
    }

    fn max_sidebar_scroll(&self) -> usize {
        self.file_count().saturating_sub(1)
    }

    fn reload_worktree(&mut self, preserve_scroll: bool) {
        match load_worktree_diff() {
            Ok(changeset) => self.apply_reloaded_changeset(changeset, preserve_scroll),
            Err(error) => self.live_error = Some(format!("reload failed: {error}")),
        }
    }

    fn apply_reloaded_changeset(&mut self, changeset: Changeset, preserve_scroll: bool) {
        let previous_identity = self.selected_file().map(file_identity);
        let previous_index = self.selected_file_index;
        let previous_scroll = self.diff_scroll;
        let reselected_file_index = previous_identity
            .as_deref()
            .and_then(|identity| find_file_index(&changeset, identity));
        let kept_selection = reselected_file_index.is_some();
        let selected_file_index = reselected_file_index
            .unwrap_or_else(|| previous_index.min(changeset.files.len().saturating_sub(1)));

        self.changeset = changeset;
        self.live_error = None;
        self.selected_file_index = selected_file_index;
        self.diff_scroll = if preserve_scroll && kept_selection {
            previous_scroll
        } else {
            0
        };
        self.diff_lines_cache = None;
        self.ensure_scroll_bounds();
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),

            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Left => self.focus = FocusPane::Sidebar,
            KeyCode::Right | KeyCode::Enter => self.focus = FocusPane::Diff,

            KeyCode::Char('j') => self.move_down(),
            KeyCode::Char('k') => self.move_up(),

            KeyCode::Home | KeyCode::Char('g') => self.diff_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll_diff_to_bottom(),

            KeyCode::Char(' ') => self.toggle_selected_file_staging()?,

            KeyCode::PageDown => self.scroll_diff_by(self.diff_view_height),
            KeyCode::PageUp => self.scroll_diff_up_by(self.diff_view_height),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_by(self.diff_view_height)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_up_by(self.diff_view_height)
            }
            _ => {}
        }

        self.ensure_scroll_bounds();
        Ok(true)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let column = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_left_click(column, row),
            MouseEventKind::ScrollDown => self.handle_wheel(column, row, WheelDirection::Down),
            MouseEventKind::ScrollUp => self.handle_wheel(column, row, WheelDirection::Up),
            MouseEventKind::Moved => self.handle_hover(column, row),
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

    fn handle_wheel(&mut self, column: u16, row: u16, direction: WheelDirection) {
        self.focus = if self.is_sidebar_at(column, row) {
            FocusPane::Sidebar
        } else {
            FocusPane::Diff
        };

        match (self.focus, direction) {
            (FocusPane::Sidebar, WheelDirection::Down) => {
                self.select_next_file_by(MOUSE_WHEEL_STEP)
            }
            (FocusPane::Sidebar, WheelDirection::Up) => {
                self.select_previous_file_by(MOUSE_WHEEL_STEP)
            }
            (FocusPane::Diff, WheelDirection::Down) => self.scroll_diff_by(MOUSE_WHEEL_STEP),
            (FocusPane::Diff, WheelDirection::Up) => self.scroll_diff_up_by(MOUSE_WHEEL_STEP),
        }
    }

    fn sidebar_index_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.sidebar_area?;
        if !rect_inner_contains(area, column, row) {
            return None;
        }

        let row_offset = row.saturating_sub(area.y + 1) as usize;
        self.sidebar_row_indices
            .get(row_offset)
            .copied()
            .filter(|index| *index < self.changeset.files.len())
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
        self.selected_file_line_count()
            .saturating_sub(self.diff_view_height.max(1))
    }

    fn toggle_selected_file_staging(&mut self) -> Result<()> {
        if self.focus != FocusPane::Sidebar || !self.changeset.source.can_stage() {
            return Ok(());
        }

        let Some(file) = self.selected_file() else {
            return Ok(());
        };

        let path = file.display_path().to_string();
        toggle_staging_for_file(&path)?;

        let reloaded_changeset = load_worktree_diff()?;
        self.apply_reloaded_changeset(reloaded_changeset, false);

        Ok(())
    }
}

struct WorktreeWatcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    root: PathBuf,
}

struct DrainedWorktreeEvents {
    changed: bool,
    error: Option<notify::Error>,
}

impl WorktreeWatcher {
    fn start() -> Result<Self> {
        let root = worktree_root()?;
        let (sender, events) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = sender.send(event);
            },
            Config::default(),
        )?;
        watcher.watch(&root, RecursiveMode::Recursive)?;

        Ok(Self {
            _watcher: watcher,
            events,
            root,
        })
    }

    fn drain(&self) -> DrainedWorktreeEvents {
        let mut changed = false;
        let mut error = None;

        while let Ok(event) = self.events.try_recv() {
            match event {
                Ok(event) if is_relevant_worktree_event(&event, &self.root) => changed = true,
                Ok(_) => {}
                Err(latest_error) => error = Some(latest_error),
            }
        }

        DrainedWorktreeEvents { changed, error }
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

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let watcher = start_live_worktree_watcher(app);
    let mut pending_reload_at: Option<Instant> = None;

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if let Some(watcher) = watcher.as_ref() {
            let drained = watcher.drain();
            if let Some(error) = drained.error {
                app.live_error = Some(format!("watch failed: {error}"));
            }
            if drained.changed {
                pending_reload_at = Some(Instant::now() + WORKTREE_RELOAD_DEBOUNCE);
            }
        }

        if pending_reload_at.is_some_and(|deadline| Instant::now() >= deadline) {
            app.reload_worktree(true);
            pending_reload_at = None;
            continue;
        }

        if !event::poll(next_event_poll_interval(pending_reload_at))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if !app.handle_key(key)? => break,
            Event::Key(_) => {}
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            _ => {}
        }
    }

    Ok(())
}

fn start_live_worktree_watcher(app: &mut App) -> Option<WorktreeWatcher> {
    if !app.changeset.source.can_stage() {
        return None;
    }

    match WorktreeWatcher::start() {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            app.live_error = Some(format!("watch failed: {error}"));
            None
        }
    }
}

fn next_event_poll_interval(pending_reload_at: Option<Instant>) -> Duration {
    let Some(deadline) = pending_reload_at else {
        return EVENT_POLL_INTERVAL;
    };

    deadline
        .saturating_duration_since(Instant::now())
        .min(EVENT_POLL_INTERVAL)
}

fn is_relevant_worktree_event(event: &notify::Event, root: &Path) -> bool {
    if !is_worktree_change_kind(&event.kind) {
        return false;
    }

    event
        .paths
        .iter()
        .any(|path| is_relevant_worktree_path(path, root))
}

fn is_worktree_change_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn is_relevant_worktree_path(path: &Path, root: &Path) -> bool {
    let Ok(relative_path) = path.strip_prefix(root) else {
        return true;
    };

    let mut components = relative_path.components();
    match components.next() {
        Some(Component::Normal(name)) if name == ".git" => {
            is_relevant_git_metadata_path(components.as_path())
        }
        _ => true,
    }
}

fn is_relevant_git_metadata_path(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Normal(name)) = components.next() else {
        return false;
    };

    match name.to_str() {
        Some("index" | "HEAD" | "packed-refs") => components.next().is_none(),
        Some("refs") => true,
        _ => false,
    }
}

fn file_identity(file: &DiffFile) -> String {
    file.display_path().to_string()
}

fn find_file_index(changeset: &Changeset, identity: &str) -> Option<usize> {
    changeset
        .files
        .iter()
        .position(|file| file.display_path() == identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiffHunk, DiffLine, DiffLineKind, DiffSource, FileStatus, SourceSnapshot};
    use crate::theme::Theme;

    #[test]
    fn diff_scroll_bounds_use_rendered_rows_when_available() {
        let mut app = App::new(changeset_with_one_file());
        app.diff_view_height = 3;
        app.diff_scroll = 99;
        app.diff_lines_cache = Some(RenderedDiffLines {
            file_id: "0".to_string(),
            content_width: 24,
            syntax_palette: Theme::github_dark().syntax,
            lines: vec![Line::raw("row"); 8],
        });

        app.ensure_scroll_bounds();

        assert_eq!(app.diff_scroll, 5);
    }

    #[test]
    fn reload_preserves_selected_file_and_scroll_by_path() {
        let mut app = App::new(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.diff_view_height = 3;
        app.diff_scroll = 4;

        app.apply_reloaded_changeset(changeset_with_paths(["b.txt", "a.txt"]), true);

        assert_eq!(
            app.selected_file().map(DiffFile::display_path),
            Some("b.txt")
        );
        assert_eq!(app.selected_file_index, 0);
        assert_eq!(app.diff_scroll, 4);
    }

    #[test]
    fn reload_clamps_scroll_when_selected_file_shrinks() {
        let mut app = App::new(changeset_with_paths(["sample.txt"]));
        app.diff_view_height = 3;
        app.diff_scroll = 99;

        app.apply_reloaded_changeset(changeset_with_short_file("sample.txt"), true);

        assert_eq!(
            app.selected_file().map(DiffFile::display_path),
            Some("sample.txt")
        );
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn reload_resets_selection_and_scroll_when_selected_file_disappears() {
        let mut app = App::new(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.diff_scroll = 4;

        app.apply_reloaded_changeset(changeset_with_paths(["a.txt"]), true);

        assert_eq!(
            app.selected_file().map(DiffFile::display_path),
            Some("a.txt")
        );
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn live_reload_treats_git_state_files_as_relevant() {
        for path in [
            ".git/index",
            ".git/HEAD",
            ".git/packed-refs",
            ".git/refs/heads/main",
            ".git/refs/remotes/origin/HEAD",
        ] {
            assert!(
                is_relevant_worktree_event(&worktree_event(path), worktree_test_root()),
                "{path} should trigger live reload",
            );
        }
    }

    #[test]
    fn live_reload_ignores_noisy_git_metadata() {
        for path in [
            ".git",
            ".git/objects/12/3456789",
            ".git/logs/HEAD",
            ".git/index.lock",
        ] {
            assert!(
                !is_relevant_worktree_event(&worktree_event(path), worktree_test_root()),
                "{path} should not trigger live reload",
            );
        }
    }

    #[test]
    fn live_reload_ignores_non_mutating_git_state_events() {
        let event = notify::Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .add_path(worktree_test_root().join(".git/index"));

        assert!(!is_relevant_worktree_event(&event, worktree_test_root()));
    }

    fn changeset_with_one_file() -> Changeset {
        changeset_with_paths(["sample.txt"])
    }

    fn worktree_event(path: &str) -> notify::Event {
        notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(worktree_test_root().join(path))
    }

    fn worktree_test_root() -> &'static Path {
        Path::new("/tmp/chunk-worktree")
    }

    fn changeset_with_short_file(path: &str) -> Changeset {
        Changeset {
            title: String::new(),
            source_label: String::new(),
            source: DiffSource::Worktree,
            files: vec![diff_file(path, 1)],
        }
    }

    fn changeset_with_paths<const N: usize>(paths: [&str; N]) -> Changeset {
        Changeset {
            title: String::new(),
            source_label: String::new(),
            source: DiffSource::Worktree,
            files: paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    let mut file = diff_file(path, 8);
                    file.id = index.to_string();
                    file
                })
                .collect(),
        }
    }

    fn diff_file(path: &str, line_count: u32) -> DiffFile {
        DiffFile {
            id: "0".to_string(),
            old_path: path.to_string(),
            path: path.to_string(),
            old_source: SourceSnapshot::Unloaded,
            new_source: SourceSnapshot::Unloaded,
            status: FileStatus::Modified,
            stage: crate::model::FileStage::Unstaged,
            additions: 0,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: format!("@@ -1,{line_count} +1,{line_count} @@"),
                old_start: 1,
                old_lines: line_count,
                new_start: 1,
                new_lines: line_count,
                lines: (1..=line_count)
                    .map(|line_number| DiffLine {
                        kind: DiffLineKind::Context,
                        old_line: Some(line_number),
                        new_line: Some(line_number),
                        content: "line".to_string(),
                    })
                    .collect(),
            }],
            binary: false,
        }
    }
}
