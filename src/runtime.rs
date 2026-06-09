//! Terminal and worktree-watch adapters for the application session.
//!
//! This module owns Crossterm, Ratatui, Notify, and event polling. It drives the
//! `App` session through behavior methods instead of editing session state.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::ui;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WORKTREE_RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);

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
    fn start(root: PathBuf) -> Result<Self> {
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

pub(crate) fn run(mut app: App) -> Result<()> {
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
    let watcher = start_live_worktree_watcher(app);
    let mut pending_reload_at: Option<Instant> = None;

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        drain_live_worktree_events(watcher.as_ref(), app, &mut pending_reload_at);
        if reload_worktree_if_due(app, &mut pending_reload_at) {
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

fn drain_live_worktree_events(
    watcher: Option<&WorktreeWatcher>,
    app: &mut App,
    pending_reload_at: &mut Option<Instant>,
) {
    let Some(watcher) = watcher else {
        return;
    };

    let DrainedWorktreeEvents { changed, error } = watcher.drain();

    if let Some(error) = error {
        app.set_live_error(format!("watch failed: {error}"));
    }

    if changed {
        *pending_reload_at = Some(Instant::now() + WORKTREE_RELOAD_DEBOUNCE);
    }
}

fn reload_worktree_if_due(app: &mut App, pending_reload_at: &mut Option<Instant>) -> bool {
    let Some(deadline) = *pending_reload_at else {
        return false;
    };

    if Instant::now() < deadline {
        return false;
    }

    app.reload_review_source(true);
    *pending_reload_at = None;
    true
}

fn start_live_worktree_watcher(app: &mut App) -> Option<WorktreeWatcher> {
    match app
        .live_watch_root()
        .and_then(|root| root.map(WorktreeWatcher::start).transpose())
    {
        Ok(watcher) => watcher,
        Err(error) => {
            app.set_live_error(format!("watch failed: {error}"));
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
    !matches!(components.next(), Some(Component::Normal(name)) if name == ".git")
        || is_relevant_git_metadata_path(components.as_path())
}

fn is_relevant_git_metadata_path(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Normal(name)) = components.next() else {
        return false;
    };

    if name == "refs" {
        return true;
    }

    (name == "index" || name == "HEAD" || name == "packed-refs") && components.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn worktree_event(path: &str) -> notify::Event {
        notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(worktree_test_root().join(path))
    }

    fn worktree_test_root() -> &'static Path {
        Path::new("/tmp/chunk-worktree")
    }
}
