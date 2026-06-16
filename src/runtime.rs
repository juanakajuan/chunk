//! Terminal and worktree-watch adapters for the application session.
//!
//! This module owns Crossterm, Ratatui, Notify, and event polling. It drives the
//! `App` session through behavior methods instead of editing session state.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::ask_ai::{self, AskAiRequest, AskAiResult};
use crate::clipboard;
use crate::custom_command::{self, CustomCommandBinding, CustomCommandResult};
use crate::editor::{EditorCommand, EditorRequest};
use crate::git;
use crate::ui;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WORKTREE_RELOAD_DEBOUNCE: Duration = Duration::from_millis(250);

type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

struct WorktreeWatcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    root: PathBuf,
}

struct DrainedWorktreeEvents {
    changed: bool,
    error: Option<notify::Error>,
}

/// A child process running on a worker thread, reporting exactly one result.
///
/// This is the single lifecycle behind every background process the session
/// launches. It owns the result channel, the cancel channel, the worker handle,
/// and the fallback result to report if the worker vanishes without answering.
/// Callers differ only in what they run and what they remember about the
/// request — see [`start_requested_custom_command`] and [`start_requested_ask_ai`].
struct BackgroundTask<T> {
    result: Receiver<T>,
    cancel: Sender<()>,
    worker: JoinHandle<()>,
    on_disconnect: Box<dyn FnOnce() -> T>,
}

impl<T: Send + 'static> BackgroundTask<T> {
    /// Spawn `run` on a worker thread, handing it a cancel receiver. If the
    /// worker disappears without sending, `on_disconnect` supplies the result.
    fn spawn(
        run: impl FnOnce(Receiver<()>) -> T + Send + 'static,
        on_disconnect: impl FnOnce() -> T + 'static,
    ) -> Self {
        let (result_sender, result) = mpsc::channel();
        let (cancel_sender, cancel) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = result_sender.send(run(cancel));
        });

        Self {
            result,
            cancel: cancel_sender,
            worker,
            on_disconnect: Box::new(on_disconnect),
        }
    }

    fn request_cancel(&self) {
        let _ = self.cancel.send(());
    }
}

/// Take a finished task's result and join its worker, or `None` while it runs.
/// A worker that vanished without sending yields the task's fallback result.
fn finish_task<T>(slot: &mut Option<BackgroundTask<T>>) -> Option<T> {
    let outcome = match slot.as_ref()?.result.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => return None,
        Err(TryRecvError::Disconnected) => None,
    };

    let task = slot.take().expect("ready task should still be stored");
    let _ = task.worker.join();
    Some(outcome.unwrap_or_else(task.on_disconnect))
}

fn cancel_and_join<T>(slot: &mut Option<BackgroundTask<T>>) {
    if let Some(task) = slot.take() {
        let _ = task.cancel.send(());
        let _ = task.worker.join();
    }
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

fn run_loop(terminal: &mut TuiTerminal, app: &mut App) -> Result<()> {
    let watcher = start_live_worktree_watcher(app);
    let mut pending_reload_at: Option<Instant> = None;
    let mut custom_command_task: Option<BackgroundTask<CustomCommandResult>> = None;
    let mut ask_ai_task: Option<BackgroundTask<AskAiResult>> = None;

    loop {
        if let Some(result) = finish_task(&mut custom_command_task) {
            app.set_custom_command_result(result);
            app.reload_review_source(true);
        }
        if let Some(result) = finish_task(&mut ask_ai_task) {
            app.set_ask_ai_result(result);
        }
        app.advance_custom_command_spinner();
        app.advance_ask_ai_spinner();
        terminal.draw(|frame| ui::draw(frame, app))?;

        drain_live_worktree_events(watcher.as_ref(), app, &mut pending_reload_at);
        if reload_worktree_if_due(app, &mut pending_reload_at) {
            continue;
        }

        if !event::poll(next_event_poll_interval(pending_reload_at))? {
            continue;
        }

        match event::read()? {
            Event::Key(key)
                if !handle_key_event(
                    terminal,
                    app,
                    key,
                    &mut custom_command_task,
                    &mut ask_ai_task,
                )? =>
            {
                cancel_and_join(&mut ask_ai_task);
                break;
            }
            Event::Key(_) => {}
            Event::Mouse(mouse) => {
                app.handle_mouse(mouse);
                write_clipboard_request(app);
            }
            _ => {}
        }
    }

    Ok(())
}

fn handle_key_event(
    terminal: &mut TuiTerminal,
    app: &mut App,
    key: KeyEvent,
    custom_command_task: &mut Option<BackgroundTask<CustomCommandResult>>,
    ask_ai_task: &mut Option<BackgroundTask<AskAiResult>>,
) -> Result<bool> {
    if !app.handle_key(key)? {
        return Ok(false);
    }

    if let Some(request) = app.take_editor_request() {
        open_requested_editor(terminal, app, &request)?;
    }

    if let Some(command) = app.take_custom_command_request() {
        start_requested_custom_command(terminal, app, command, custom_command_task)?;
    }

    if app.take_ask_ai_cancel_request()
        && let Some(task) = ask_ai_task.as_ref()
    {
        task.request_cancel();
    }

    if let Some(request) = app.take_ask_ai_request() {
        start_requested_ask_ai(terminal, app, request, ask_ai_task)?;
    }

    Ok(true)
}

fn write_clipboard_request(app: &mut App) {
    let Some(text) = app.take_clipboard_request() else {
        return;
    };

    if let Err(error) = clipboard::write_text(&text) {
        app.set_live_error(format!("copy failed: {error}"));
    }
}

fn open_requested_editor(
    terminal: &mut TuiTerminal,
    app: &mut App,
    request: &EditorRequest,
) -> Result<()> {
    match open_editor(terminal, request)? {
        Some(error) => app.set_live_error(error),
        None => app.reload_review_source(true),
    }

    Ok(())
}

fn open_editor(terminal: &mut TuiTerminal, request: &EditorRequest) -> Result<Option<String>> {
    let editor = match EditorCommand::from_env() {
        Ok(editor) => editor,
        Err(error) => return Ok(Some(error)),
    };

    suspend_terminal(terminal)?;
    let status = editor.status(request);
    resume_terminal(terminal)?;
    terminal.clear()?;

    match status {
        Ok(status) if status.success() => Ok(None),
        Ok(status) => Ok(Some(format!("editor exited with status {status}"))),
        Err(error) => Ok(Some(format!(
            "failed to start editor `{}`: {error}",
            editor.display_name()
        ))),
    }
}

fn start_requested_custom_command(
    terminal: &mut TuiTerminal,
    app: &mut App,
    command: CustomCommandBinding,
    custom_command_task: &mut Option<BackgroundTask<CustomCommandResult>>,
) -> Result<()> {
    if custom_command_task.is_some() {
        return Ok(());
    }

    app.set_custom_command_running(&command);
    terminal.draw(|frame| ui::draw(frame, app))?;

    let run_command = command.clone();
    *custom_command_task = Some(BackgroundTask::spawn(
        move |_cancel| run_custom_command(&run_command),
        move || {
            CustomCommandResult::not_started(
                &command,
                None,
                "command runner stopped before reporting a result",
            )
        },
    ));
    Ok(())
}

fn start_requested_ask_ai(
    terminal: &mut TuiTerminal,
    app: &mut App,
    request: AskAiRequest,
    ask_ai_task: &mut Option<BackgroundTask<AskAiResult>>,
) -> Result<()> {
    if ask_ai_task.is_some() {
        return Ok(());
    }

    app.set_ask_ai_running(&request);
    terminal.draw(|frame| ui::draw(frame, app))?;

    let run_request = request.clone();
    *ask_ai_task = Some(BackgroundTask::spawn(
        move |cancel| run_ask_ai_request(run_request, cancel),
        move || {
            AskAiResult::not_started(
                request,
                None,
                "OpenCode runner stopped before reporting a result",
            )
        },
    ));
    Ok(())
}

fn run_ask_ai_request(request: AskAiRequest, cancel: Receiver<()>) -> AskAiResult {
    let repo_root = match git::worktree_root() {
        Ok(root) => root,
        Err(error) => {
            return AskAiResult::not_started(
                request,
                None,
                format!("could not determine Git worktree root: {error}"),
            );
        }
    };

    ask_ai::run(request.clone(), repo_root.clone(), cancel).unwrap_or_else(|error| {
        AskAiResult::not_started(request, Some(repo_root), error.to_string())
    })
}

fn run_custom_command(command: &CustomCommandBinding) -> CustomCommandResult {
    let cwd = match git::worktree_root() {
        Ok(root) => root,
        Err(error) => {
            return CustomCommandResult::not_started(
                command,
                None,
                format!("could not determine Git worktree root: {error}"),
            );
        }
    };

    custom_command::run(command, cwd.clone()).unwrap_or_else(|error| {
        CustomCommandResult::not_started(command, Some(cwd), error.to_string())
    })
}

fn suspend_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
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
