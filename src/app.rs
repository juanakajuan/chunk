//! Terminal application session state.
//!
//! `App` owns selection, focus, scroll state, and live reload errors. Review
//! source behavior lives in `review_source`; terminal and watch orchestration
//! live in `runtime`; rendering is delegated to `ui`.

use std::path::PathBuf;

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::model::{Changeset, DiffFile};
use crate::review_source::{LoadedReview, ReviewSource};
use crate::viewport::{RenderedViewport, ViewportScrollInput};

const MOUSE_WHEEL_STEP: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Sidebar,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerticalDirection {
    Down,
    Up,
}

#[derive(Debug)]
pub(crate) struct App {
    /// Review source behavior for this session.
    source: ReviewSource,
    /// Current diff data being reviewed.
    pub(crate) changeset: Changeset,
    /// Last live reload/watch error, rendered above the diff when present.
    pub(crate) live_error: Option<String>,
    /// Index into `changeset.files`.
    pub(crate) selected_file_index: usize,
    /// Pane receiving keyboard and mouse wheel actions.
    pub(crate) focus: FocusPane,
    /// Whether the files sidebar is visible in the current session.
    pub(crate) files_panel_visible: bool,
    /// First rendered diff row visible in the diff pane.
    pub(crate) diff_scroll: usize,
    /// First file index considered for sidebar rendering.
    pub(crate) sidebar_scroll: usize,
    /// Rendered viewport geometry, row mapping, and render caches.
    pub(crate) viewport: RenderedViewport,
}

impl App {
    pub(crate) fn new(review: LoadedReview) -> Self {
        let LoadedReview { source, changeset } = review;
        let file_count = changeset.files.len();
        Self {
            source,
            changeset,
            live_error: None,
            selected_file_index: 0,
            focus: FocusPane::Sidebar,
            files_panel_visible: true,
            diff_scroll: 0,
            sidebar_scroll: 0,
            viewport: RenderedViewport::new(file_count),
        }
    }

    pub(crate) fn can_stage(&self) -> bool {
        self.source.can_stage()
    }

    pub(crate) fn live_watch_root(&self) -> Result<Option<PathBuf>> {
        self.source.live_watch_root()
    }

    pub(crate) fn empty_sidebar_message(&self) -> &'static str {
        self.source.empty_sidebar_message()
    }

    pub(crate) fn no_diff_message(&self) -> &'static str {
        self.source.no_diff_message()
    }

    pub(crate) fn selected_file(&self) -> Option<&DiffFile> {
        self.changeset.files.get(self.selected_file_index)
    }

    pub(crate) fn ensure_selected_file_sources_loaded(&mut self) {
        let source = &self.source;
        if let Some(file) = self.changeset.files.get_mut(self.selected_file_index) {
            source.load_source_snapshots(file);
        }
    }

    pub(crate) fn ensure_scroll_bounds(&mut self) {
        let scrolls = self.viewport.clamped_scrolls(self.viewport_scroll_input());
        self.diff_scroll = scrolls.diff_scroll;
        self.sidebar_scroll = scrolls.sidebar_scroll;
    }

    fn viewport_scroll_input(&self) -> ViewportScrollInput<'_> {
        let selected_file = self.selected_file();
        ViewportScrollInput {
            diff_scroll: self.diff_scroll,
            sidebar_scroll: self.sidebar_scroll,
            selected_file_index: self.selected_file_index,
            file_count: self.changeset.files.len(),
            selected_file_id: selected_file.map(|file| file.id.as_str()),
            selected_file_line_count: selected_file.map_or(0, DiffFile::line_count),
        }
    }

    pub(crate) fn set_live_error(&mut self, error: String) {
        self.live_error = Some(error);
    }

    pub(crate) fn reload_review_source(&mut self, preserve_scroll: bool) {
        match self.source.reload() {
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
        let fallback_index = previous_index.min(changeset.files.len().saturating_sub(1));
        let kept_selection = reselected_file_index.is_some();
        let selected_file_index = reselected_file_index.unwrap_or(fallback_index);

        self.changeset = changeset;
        self.live_error = None;
        self.selected_file_index = selected_file_index;
        self.diff_scroll = if preserve_scroll && kept_selection {
            previous_scroll
        } else {
            0
        };
        self.clear_render_caches();
        self.ensure_scroll_bounds();
    }

    fn clear_render_caches(&mut self) {
        self.viewport
            .clear_render_caches(self.changeset.files.len());
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),

            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Char('f') => self.toggle_files_panel(),
            KeyCode::Left if self.files_panel_visible => self.focus = FocusPane::Sidebar,
            KeyCode::Right | KeyCode::Enter => self.focus = FocusPane::Diff,

            KeyCode::Char('j') => self.move_by(VerticalDirection::Down),
            KeyCode::Char('k') => self.move_by(VerticalDirection::Up),

            KeyCode::Home | KeyCode::Char('g') => self.diff_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll_diff_to_bottom(),

            KeyCode::Char(' ') => self.toggle_selected_file_staging()?,

            KeyCode::PageDown => self.scroll_diff_page(VerticalDirection::Down),
            KeyCode::PageUp => self.scroll_diff_page(VerticalDirection::Up),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_page(VerticalDirection::Down)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_diff_page(VerticalDirection::Up)
            }
            _ => {}
        }

        self.ensure_scroll_bounds();
        Ok(true)
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        let column = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_left_click(column, row),
            MouseEventKind::ScrollDown => self.handle_wheel(column, row, VerticalDirection::Down),
            MouseEventKind::ScrollUp => self.handle_wheel(column, row, VerticalDirection::Up),
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
        if let Some(focus) = self.pane_at(column, row) {
            self.focus = focus;
        }
    }

    fn handle_wheel(&mut self, column: u16, row: u16, direction: VerticalDirection) {
        let focus = self.pane_at(column, row).unwrap_or(FocusPane::Diff);
        self.focus = focus;

        match focus {
            FocusPane::Sidebar => self.select_file_by(direction, MOUSE_WHEEL_STEP),
            FocusPane::Diff => self.scroll_diff_by(direction, MOUSE_WHEEL_STEP),
        }
    }

    fn pane_at(&self, column: u16, row: u16) -> Option<FocusPane> {
        if self.is_sidebar_at(column, row) {
            Some(FocusPane::Sidebar)
        } else if self.is_diff_at(column, row) {
            Some(FocusPane::Diff)
        } else {
            None
        }
    }

    fn sidebar_index_at(&self, column: u16, row: u16) -> Option<usize> {
        self.viewport
            .sidebar_index_at(column, row, self.changeset.files.len())
    }

    fn is_sidebar_at(&self, column: u16, row: u16) -> bool {
        self.viewport.is_sidebar_at(column, row)
    }

    fn is_diff_at(&self, column: u16, row: u16) -> bool {
        self.viewport.is_diff_at(column, row)
    }

    fn toggle_focus(&mut self) {
        if !self.files_panel_visible {
            self.focus = FocusPane::Diff;
            return;
        }

        self.focus = match self.focus {
            FocusPane::Sidebar => FocusPane::Diff,
            FocusPane::Diff => FocusPane::Sidebar,
        };
    }

    fn toggle_files_panel(&mut self) {
        self.files_panel_visible = !self.files_panel_visible;
        self.focus = if self.files_panel_visible {
            FocusPane::Sidebar
        } else {
            FocusPane::Diff
        };
    }

    fn move_by(&mut self, direction: VerticalDirection) {
        match self.focus {
            FocusPane::Sidebar => self.select_file_by(direction, 1),
            FocusPane::Diff => self.scroll_diff_by(direction, 1),
        }
    }

    fn select_file_by(&mut self, direction: VerticalDirection, amount: usize) {
        let index = match direction {
            VerticalDirection::Down => {
                let max_index = self.changeset.files.len().saturating_sub(1);
                self.selected_file_index
                    .saturating_add(amount)
                    .min(max_index)
            }
            VerticalDirection::Up => self.selected_file_index.saturating_sub(amount),
        };

        self.select_file(index);
    }

    fn select_file(&mut self, index: usize) {
        if self.changeset.files.is_empty() {
            return;
        }

        self.selected_file_index = index.min(self.changeset.files.len() - 1);
        self.diff_scroll = 0;
    }

    fn scroll_diff_page(&mut self, direction: VerticalDirection) {
        self.scroll_diff_by(direction, self.viewport.diff_view_height());
    }

    fn scroll_diff_by(&mut self, direction: VerticalDirection, amount: usize) {
        self.diff_scroll = match direction {
            VerticalDirection::Down => self.diff_scroll.saturating_add(amount),
            VerticalDirection::Up => self.diff_scroll.saturating_sub(amount),
        };
    }

    fn scroll_diff_to_bottom(&mut self) {
        self.diff_scroll = usize::MAX;
    }

    fn toggle_selected_file_staging(&mut self) -> Result<()> {
        if self.focus != FocusPane::Sidebar {
            return Ok(());
        }

        let Some(file) = self.selected_file() else {
            return Ok(());
        };

        let path = file.display_path().to_string();
        if let Some(reloaded_changeset) = self.source.toggle_staging_for_file(&path)? {
            self.apply_reloaded_changeset(reloaded_changeset, false);
        }

        Ok(())
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
    use crate::model::{DiffHunk, DiffLine, DiffLineKind, FileStatus, SourceSnapshot};
    use crate::theme::Theme;
    use crate::viewport::RenderedDiffLines;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    #[test]
    fn diff_scroll_bounds_use_rendered_rows_when_available() {
        let mut app = app_with(changeset_with_one_file());
        app.viewport.begin_diff(Rect::default(), 3);
        app.diff_scroll = 99;
        app.viewport.cache_diff_lines(
            0,
            RenderedDiffLines::new(
                "0".to_string(),
                24,
                Theme::github_dark().syntax,
                true,
                vec![Line::raw("row"); 8],
                true,
            ),
        );

        app.ensure_scroll_bounds();

        assert_eq!(app.diff_scroll, 5);
    }

    #[test]
    fn reload_preserves_selected_file_and_scroll_by_path() {
        let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.viewport.begin_diff(Rect::default(), 3);
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
        let mut app = app_with(changeset_with_paths(["sample.txt"]));
        app.viewport.begin_diff(Rect::default(), 3);
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
        let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
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
    fn hiding_files_panel_moves_focus_to_diff() {
        let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
        app.selected_file_index = 1;
        app.sidebar_scroll = 1;
        app.diff_scroll = 3;

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        assert!(!app.files_panel_visible);
        assert_eq!(app.focus, FocusPane::Diff);
        assert_eq!(app.selected_file_index, 1);
        assert_eq!(app.sidebar_scroll, 1);
        assert_eq!(app.diff_scroll, 3);
    }

    #[test]
    fn hidden_files_panel_cannot_receive_keyboard_focus() {
        let mut app = app_with(changeset_with_one_file());
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focus, FocusPane::Diff);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focus, FocusPane::Diff);
    }

    #[test]
    fn showing_files_panel_moves_focus_to_sidebar() {
        let mut app = app_with(changeset_with_one_file());
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        assert!(app.files_panel_visible);
        assert_eq!(app.focus, FocusPane::Sidebar);
    }

    fn app_with(changeset: Changeset) -> App {
        App::new(LoadedReview::worktree(changeset))
    }

    fn changeset_with_one_file() -> Changeset {
        changeset_with_paths(["sample.txt"])
    }

    fn changeset_with_short_file(path: &str) -> Changeset {
        Changeset {
            title: String::new(),
            source_label: String::new(),
            files: vec![diff_file(path, 1)],
        }
    }

    fn changeset_with_paths<const N: usize>(paths: [&str; N]) -> Changeset {
        Changeset {
            title: String::new(),
            source_label: String::new(),
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
