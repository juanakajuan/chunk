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
use crossterm::cursor::Show;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{
    App, AppEffect, ClipboardRequest, ClipboardWriteResult, EditorOutcome, RuntimeEvent,
};
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

#[derive(Debug, Default)]
struct TerminalRestore {
    raw_mode_enabled: bool,
    alternate_screen_enabled: bool,
    mouse_capture_enabled: bool,
}

trait TerminalCommands {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn enable_mouse_capture(&mut self) -> io::Result<()>;
    fn disable_mouse_capture(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
}

struct CrosstermTerminalCommands<'a> {
    stdout: &'a mut io::Stdout,
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

impl TerminalRestore {
    fn restore(&mut self, commands: &mut impl TerminalCommands) -> io::Result<()> {
        let should_show_cursor =
            self.raw_mode_enabled || self.alternate_screen_enabled || self.mouse_capture_enabled;
        let mut first_error = None;

        if self.raw_mode_enabled
            && remember_terminal_result(&mut first_error, commands.disable_raw_mode())
        {
            self.raw_mode_enabled = false;
        }
        if self.alternate_screen_enabled
            && remember_terminal_result(&mut first_error, commands.leave_alternate_screen())
        {
            self.alternate_screen_enabled = false;
        }
        if self.mouse_capture_enabled
            && remember_terminal_result(&mut first_error, commands.disable_mouse_capture())
        {
            self.mouse_capture_enabled = false;
        }
        if should_show_cursor {
            remember_terminal_result(&mut first_error, commands.show_cursor());
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let mut commands = CrosstermTerminalCommands {
            stdout: &mut stdout,
        };
        let _ = self.restore(&mut commands);
    }
}

impl TerminalCommands for CrosstermTerminalCommands<'_> {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(&mut *self.stdout, EnterAlternateScreen)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(&mut *self.stdout, LeaveAlternateScreen)
    }

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        execute!(&mut *self.stdout, EnableMouseCapture)
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        execute!(&mut *self.stdout, DisableMouseCapture)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(&mut *self.stdout, Show)
    }
}

fn enter_terminal_mode(commands: &mut impl TerminalCommands) -> io::Result<TerminalRestore> {
    let mut restore = TerminalRestore::default();

    commands.enable_raw_mode()?;
    restore.raw_mode_enabled = true;

    if let Err(error) = commands.enter_alternate_screen() {
        let _ = restore.restore(commands);
        return Err(error);
    }
    restore.alternate_screen_enabled = true;

    if let Err(error) = commands.enable_mouse_capture() {
        let _ = restore.restore(commands);
        return Err(error);
    }
    restore.mouse_capture_enabled = true;

    Ok(restore)
}

fn remember_terminal_result(first_error: &mut Option<io::Error>, result: io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            if first_error.is_none() {
                *first_error = Some(error);
            }
            false
        }
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
    let mut stdout = io::stdout();
    let mut terminal_commands = CrosstermTerminalCommands {
        stdout: &mut stdout,
    };
    let mut terminal_restore = enter_terminal_mode(&mut terminal_commands)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, &mut app);

    let mut stdout = io::stdout();
    let mut terminal_commands = CrosstermTerminalCommands {
        stdout: &mut stdout,
    };
    terminal_restore.restore(&mut terminal_commands)?;

    result
}

fn run_loop(terminal: &mut TuiTerminal, app: &mut App) -> Result<()> {
    let watcher = start_live_worktree_watcher(app);
    let mut pending_reload_at: Option<Instant> = None;
    let mut custom_command_task: Option<BackgroundTask<CustomCommandResult>> = None;
    let mut ask_ai_task: Option<BackgroundTask<AskAiResult>> = None;

    loop {
        if let Some(result) = finish_task(&mut custom_command_task) {
            app.handle_runtime_event(RuntimeEvent::CustomCommandFinished(result));
        }
        if let Some(result) = finish_task(&mut ask_ai_task) {
            app.handle_runtime_event(RuntimeEvent::AskAiFinished(result));
        }
        app.advance_custom_command_spinner();
        app.advance_ask_ai_spinner();
        terminal.draw(|frame| ui::draw(frame, app))?;

        if let Some(event) = drain_live_worktree_events(watcher.as_ref(), &mut pending_reload_at) {
            app.handle_runtime_event(event);
        }
        if let Some(event) = reload_worktree_if_due(&mut pending_reload_at) {
            app.handle_runtime_event(event);
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
                handle_app_effects(terminal, app, &mut custom_command_task, &mut ask_ai_task)?;
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

    handle_app_effects(terminal, app, custom_command_task, ask_ai_task)?;

    Ok(true)
}

fn handle_app_effects(
    terminal: &mut TuiTerminal,
    app: &mut App,
    custom_command_task: &mut Option<BackgroundTask<CustomCommandResult>>,
    ask_ai_task: &mut Option<BackgroundTask<AskAiResult>>,
) -> Result<()> {
    for effect in app.take_effects() {
        match effect {
            AppEffect::OpenEditor(request) => {
                let event = open_requested_editor(terminal, &request)?;
                app.handle_runtime_event(event);
            }
            AppEffect::RunCustomCommand(command) => {
                start_requested_custom_command(command, custom_command_task);
            }
            AppEffect::CancelCustomCommand => {
                if let Some(task) = custom_command_task.as_ref() {
                    task.request_cancel();
                }
            }
            AppEffect::RunAskAi(request) => {
                start_requested_ask_ai(request, ask_ai_task);
            }
            AppEffect::RunUnpublishedSummary => {
                start_requested_unpublished_summary(ask_ai_task);
            }
            AppEffect::CancelAskAi => {
                if let Some(task) = ask_ai_task.as_ref() {
                    task.request_cancel();
                }
            }
            AppEffect::CopyToClipboard(request) => {
                app.handle_runtime_event(write_clipboard_request(request));
            }
        }
    }

    Ok(())
}

fn write_clipboard_request(request: ClipboardRequest) -> RuntimeEvent {
    let result = match clipboard::write_text(request.text()) {
        Ok(()) => ClipboardWriteResult::completed(request),
        Err(error) => ClipboardWriteResult::failed(request, error.to_string()),
    };
    RuntimeEvent::ClipboardWriteFinished(result)
}

fn open_requested_editor(
    terminal: &mut TuiTerminal,
    request: &EditorRequest,
) -> Result<RuntimeEvent> {
    Ok(RuntimeEvent::EditorFinished(open_editor(
        terminal, request,
    )?))
}

fn open_editor(terminal: &mut TuiTerminal, request: &EditorRequest) -> Result<EditorOutcome> {
    let editor = match EditorCommand::from_env() {
        Ok(editor) => editor,
        Err(error) => return Ok(EditorOutcome::Failed(error)),
    };

    suspend_terminal(terminal)?;
    let status = editor.status(request);
    resume_terminal(terminal)?;
    terminal.clear()?;

    match status {
        Ok(status) if status.success() => Ok(EditorOutcome::Completed),
        Ok(status) => Ok(EditorOutcome::Failed(format!(
            "editor exited with status {status}"
        ))),
        Err(error) => Ok(EditorOutcome::Failed(format!(
            "failed to start editor `{}`: {error}",
            editor.display_name()
        ))),
    }
}

fn start_requested_custom_command(
    command: CustomCommandBinding,
    custom_command_task: &mut Option<BackgroundTask<CustomCommandResult>>,
) {
    if custom_command_task.is_some() {
        return;
    }

    let run_command = command.clone();
    *custom_command_task = Some(BackgroundTask::spawn(
        move |cancel| run_custom_command(&run_command, cancel),
        move || {
            CustomCommandResult::not_started(
                &command,
                None,
                "command runner stopped before reporting a result",
            )
        },
    ));
}

fn start_requested_ask_ai(
    request: AskAiRequest,
    ask_ai_task: &mut Option<BackgroundTask<AskAiResult>>,
) {
    if ask_ai_task.is_some() {
        return;
    }

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
}

fn start_requested_unpublished_summary(ask_ai_task: &mut Option<BackgroundTask<AskAiResult>>) {
    if ask_ai_task.is_some() {
        return;
    }

    *ask_ai_task = Some(BackgroundTask::spawn(
        run_unpublished_summary_request,
        || {
            AskAiResult::unpublished_summary_not_started(
                None,
                "OpenCode runner stopped before reporting a result",
            )
        },
    ));
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

fn run_unpublished_summary_request(cancel: Receiver<()>) -> AskAiResult {
    let diff = match git::load_unpublished_diff_text() {
        Ok(diff) => diff,
        Err(error) => {
            return AskAiResult::unpublished_summary_not_started(
                None,
                format!("could not load unpublished diff: {error}"),
            );
        }
    };

    if diff.text.trim().is_empty() {
        return AskAiResult::unpublished_summary_message(
            diff.repo_root,
            "No unpublished changes found.",
        );
    }

    ask_ai::run_unpublished_summary(&diff.text, diff.repo_root.clone(), cancel).unwrap_or_else(
        |error| {
            AskAiResult::unpublished_summary_not_started(Some(diff.repo_root), error.to_string())
        },
    )
}

fn run_custom_command(command: &CustomCommandBinding, cancel: Receiver<()>) -> CustomCommandResult {
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

    custom_command::run(command, cwd.clone(), cancel).unwrap_or_else(|error| {
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
    pending_reload_at: &mut Option<Instant>,
) -> Option<RuntimeEvent> {
    let watcher = watcher?;

    let DrainedWorktreeEvents { changed, error } = watcher.drain();

    if changed {
        *pending_reload_at = Some(Instant::now() + WORKTREE_RELOAD_DEBOUNCE);
    }

    error.map(|error| RuntimeEvent::LiveWatchFailed(error.to_string()))
}

fn reload_worktree_if_due(pending_reload_at: &mut Option<Instant>) -> Option<RuntimeEvent> {
    let deadline = (*pending_reload_at)?;

    if Instant::now() < deadline {
        return None;
    }

    *pending_reload_at = None;
    Some(RuntimeEvent::ReloadReviewSource {
        preserve_scroll: true,
    })
}

fn start_live_worktree_watcher(app: &mut App) -> Option<WorktreeWatcher> {
    match app
        .live_watch_root()
        .and_then(|root| root.map(WorktreeWatcher::start).transpose())
    {
        Ok(watcher) => watcher,
        Err(error) => {
            app.handle_runtime_event(RuntimeEvent::LiveWatchFailed(error.to_string()));
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TerminalAction {
        EnableRawMode,
        DisableRawMode,
        EnterAlternateScreen,
        LeaveAlternateScreen,
        EnableMouseCapture,
        DisableMouseCapture,
        ShowCursor,
    }

    #[derive(Debug, Default)]
    struct FakeTerminalCommands {
        actions: Vec<TerminalAction>,
        fail_on: Option<TerminalAction>,
    }

    impl FakeTerminalCommands {
        fn failing_on(action: TerminalAction) -> Self {
            Self {
                actions: Vec::new(),
                fail_on: Some(action),
            }
        }

        fn record(&mut self, action: TerminalAction) -> io::Result<()> {
            self.actions.push(action);
            if self.fail_on == Some(action) {
                return Err(io::Error::other(format!("{action:?} failed")));
            }
            Ok(())
        }
    }

    impl TerminalCommands for FakeTerminalCommands {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.record(TerminalAction::EnableRawMode)
        }

        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.record(TerminalAction::DisableRawMode)
        }

        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.record(TerminalAction::EnterAlternateScreen)
        }

        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.record(TerminalAction::LeaveAlternateScreen)
        }

        fn enable_mouse_capture(&mut self) -> io::Result<()> {
            self.record(TerminalAction::EnableMouseCapture)
        }

        fn disable_mouse_capture(&mut self) -> io::Result<()> {
            self.record(TerminalAction::DisableMouseCapture)
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.record(TerminalAction::ShowCursor)
        }
    }

    #[test]
    fn terminal_mode_setup_restores_partial_state_when_mouse_capture_fails() {
        let mut commands = FakeTerminalCommands::failing_on(TerminalAction::EnableMouseCapture);

        let error = enter_terminal_mode(&mut commands).unwrap_err();

        assert!(error.to_string().contains("EnableMouseCapture failed"));
        assert_eq!(
            commands.actions,
            [
                TerminalAction::EnableRawMode,
                TerminalAction::EnterAlternateScreen,
                TerminalAction::EnableMouseCapture,
                TerminalAction::DisableRawMode,
                TerminalAction::LeaveAlternateScreen,
                TerminalAction::ShowCursor,
            ]
        );
    }

    #[test]
    fn terminal_restore_cleans_up_successful_setup_once() {
        let mut commands = FakeTerminalCommands::default();
        let mut restore = enter_terminal_mode(&mut commands).unwrap();

        restore.restore(&mut commands).unwrap();
        restore.restore(&mut commands).unwrap();

        assert_eq!(
            commands.actions,
            [
                TerminalAction::EnableRawMode,
                TerminalAction::EnterAlternateScreen,
                TerminalAction::EnableMouseCapture,
                TerminalAction::DisableRawMode,
                TerminalAction::LeaveAlternateScreen,
                TerminalAction::DisableMouseCapture,
                TerminalAction::ShowCursor,
            ]
        );
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

    fn worktree_event(path: &str) -> notify::Event {
        notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(worktree_test_root().join(path))
    }

    fn worktree_test_root() -> &'static Path {
        Path::new("/tmp/chunk-worktree")
    }
}
