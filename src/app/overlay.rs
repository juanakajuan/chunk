use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::text::Line;

use crate::ask_ai::{AskAiContext, AskAiRequest, AskAiResult};
use crate::custom_command::{CustomCommandBinding, CustomCommandResult};
use crate::keybind::BuiltinAction;
use crate::rows;
use crate::scroll_text::ScrollText;
use crate::theme::Theme;

use super::keys::{
    apply_scroll_key, closes_ask_ai_output, closes_ask_ai_running, closes_command_output,
    closes_help_overlay,
};
use super::{App, FocusPane, HELP_OVERLAY_SCROLL_PAGE, MOUSE_WHEEL_STEP, accepts_text_input};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandOutputState {
    pub(super) result: CustomCommandResult,
    pub(super) scroll: ScrollText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AskAiPromptState {
    pub(super) context: AskAiContext,
    pub(super) input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AskAiOutputState {
    pub(super) result: AskAiResult,
    pub(super) scroll: ScrollText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiscardConfirmation {
    pub(super) target: DiscardTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DiscardTarget {
    File {
        file_index: usize,
        path: String,
    },
    Folder {
        path: String,
        file_paths: Vec<String>,
    },
    Hunk {
        file_index: usize,
        hunk_index: usize,
        path: String,
    },
}

impl DiscardConfirmation {
    fn prompt(&self) -> String {
        match &self.target {
            DiscardTarget::File { path, .. } => {
                format!("Discard worktree changes in {path}?")
            }
            DiscardTarget::Folder { path, file_paths } => {
                let file_count = file_paths.len();
                let noun = if file_count == 1 { "file" } else { "files" };
                format!("Discard worktree changes in {path}/ ({file_count} {noun})?")
            }
            DiscardTarget::Hunk {
                hunk_index, path, ..
            } => format!("Discard hunk {} in {path}?", hunk_index + 1),
        }
    }
}

/// The single modal overlay active over the diff view, if any.
///
/// At most one overlay can be active at a time. Holding them in one value makes
/// that exclusivity a type invariant instead of a guard-clause ordering that
/// would otherwise be re-derived across `handle_key`, `handle_mouse`, and the
/// renderer. The literal search prompt is intentionally not an overlay: it
/// captures keystrokes only, leaves mouse handling identical to the normal diff
/// view, and its persistent query/match state belongs to the `search` module.
#[derive(Debug)]
pub(super) enum Overlay {
    /// Keymap help modal; owns the scroll offset for keymaps taller than the modal.
    Help { scroll: ScrollText },
    /// Pending destructive worktree discard awaiting y/n confirmation.
    Discard(DiscardConfirmation),
    /// A custom command is running; all input is swallowed until runtime delivers output.
    CommandRunning {
        binding: CustomCommandBinding,
        spinner_frame: usize,
    },
    /// Completed custom command output shown in the diff pane.
    CommandOutput(CommandOutputState),
    /// Free-form Ask AI prompt; owns input until submitted.
    AskAiPrompt(AskAiPromptState),
    /// Ask AI request is running in the background.
    AskAiRunning {
        question: String,
        spinner_frame: usize,
        cancelling: bool,
    },
    /// Completed Ask AI answer shown in the diff pane.
    AskAiOutput(AskAiOutputState),
}

impl App {
    pub(crate) fn help_overlay_visible(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Help { .. }))
    }

    pub(crate) fn help_overlay_scroll(&self) -> usize {
        match &self.overlay {
            Some(Overlay::Help { scroll }) => scroll.offset(),
            _ => 0,
        }
    }

    pub(crate) fn clamp_help_overlay_scroll(&mut self, line_count: usize, visible_height: usize) {
        if let Some(scroll) = self.help_scroll_mut() {
            scroll.sync(line_count, visible_height);
        }
    }

    pub(super) fn command_output(&self) -> Option<&CommandOutputState> {
        match &self.overlay {
            Some(Overlay::CommandOutput(output)) => Some(output),
            _ => None,
        }
    }

    pub(super) fn command_running(&self) -> Option<(&CustomCommandBinding, usize)> {
        match &self.overlay {
            Some(Overlay::CommandRunning {
                binding,
                spinner_frame,
            }) => Some((binding, *spinner_frame)),
            _ => None,
        }
    }

    pub(super) fn ask_ai_prompt(&self) -> Option<&AskAiPromptState> {
        match &self.overlay {
            Some(Overlay::AskAiPrompt(prompt)) => Some(prompt),
            _ => None,
        }
    }

    pub(super) fn ask_ai_running(&self) -> Option<(&str, usize, bool)> {
        match &self.overlay {
            Some(Overlay::AskAiRunning {
                question,
                spinner_frame,
                cancelling,
            }) => Some((question.as_str(), *spinner_frame, *cancelling)),
            _ => None,
        }
    }

    pub(super) fn ask_ai_output(&self) -> Option<&AskAiOutputState> {
        match &self.overlay {
            Some(Overlay::AskAiOutput(output)) => Some(output),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn discard_target(&self) -> Option<&DiscardTarget> {
        match &self.overlay {
            Some(Overlay::Discard(confirmation)) => Some(&confirmation.target),
            _ => None,
        }
    }

    pub(super) fn discard_status_lines(
        &self,
        content_width: usize,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let prompt = match &self.overlay {
            Some(Overlay::Discard(confirmation)) => Some(confirmation.prompt()),
            _ => None,
        };
        rows::discard_status_lines(prompt.as_deref(), content_width, theme)
    }

    pub(crate) fn set_custom_command_running(&mut self, command: &CustomCommandBinding) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::CommandRunning {
            binding: command.clone(),
            spinner_frame: 0,
        });
    }

    pub(crate) fn advance_custom_command_spinner(&mut self) {
        if let Some(Overlay::CommandRunning { spinner_frame, .. }) = &mut self.overlay {
            *spinner_frame = spinner_frame.wrapping_add(1);
        }
    }

    pub(crate) fn set_custom_command_result(&mut self, result: CustomCommandResult) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::CommandOutput(CommandOutputState {
            result,
            scroll: ScrollText::default(),
        }));
    }

    pub(crate) fn set_ask_ai_running(&mut self, request: &AskAiRequest) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::AskAiRunning {
            question: request.question().to_string(),
            spinner_frame: 0,
            cancelling: false,
        });
    }

    pub(crate) fn advance_ask_ai_spinner(&mut self) {
        if let Some(Overlay::AskAiRunning { spinner_frame, .. }) = &mut self.overlay {
            *spinner_frame = spinner_frame.wrapping_add(1);
        }
    }

    pub(crate) fn set_ask_ai_result(&mut self, result: AskAiResult) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(Overlay::AskAiOutput(AskAiOutputState {
            result,
            scroll: ScrollText::default(),
        }));
    }

    pub(super) fn handle_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay {
            Some(Overlay::Help { .. }) => self.handle_help_overlay_key(key),
            Some(Overlay::Discard(_)) => self.handle_discard_confirmation_key(key),
            Some(Overlay::CommandOutput(_)) => self.handle_command_output_key(key),
            Some(Overlay::AskAiPrompt(_)) => self.handle_ask_ai_prompt_key(key),
            Some(Overlay::AskAiRunning { .. }) => self.handle_ask_ai_running_key(key),
            Some(Overlay::AskAiOutput(_)) => self.handle_ask_ai_output_key(key),
            Some(Overlay::CommandRunning { .. }) | None => {}
        }
    }

    pub(super) fn handle_overlay_mouse(&mut self, mouse: MouseEvent) {
        match self.overlay {
            Some(Overlay::Help { .. }) => self.handle_help_overlay_mouse(mouse),
            Some(Overlay::CommandOutput(_)) => self.handle_command_output_mouse(mouse),
            Some(Overlay::AskAiOutput(_)) => self.handle_ask_ai_output_mouse(mouse),
            Some(Overlay::Discard(_))
            | Some(Overlay::CommandRunning { .. })
            | Some(Overlay::AskAiPrompt(_))
            | Some(Overlay::AskAiRunning { .. })
            | None => {}
        }
    }

    pub(super) fn toggle_help_overlay(&mut self) {
        self.overlay = match self.overlay {
            Some(Overlay::Help { .. }) => None,
            _ => Some(Overlay::Help {
                scroll: ScrollText::default(),
            }),
        };
    }

    fn help_scroll_mut(&mut self) -> Option<&mut ScrollText> {
        match &mut self.overlay {
            Some(Overlay::Help { scroll }) => Some(scroll),
            _ => None,
        }
    }

    pub(super) fn command_output_mut(&mut self) -> Option<&mut CommandOutputState> {
        match &mut self.overlay {
            Some(Overlay::CommandOutput(output)) => Some(output),
            _ => None,
        }
    }

    fn ask_ai_prompt_mut(&mut self) -> Option<&mut AskAiPromptState> {
        match &mut self.overlay {
            Some(Overlay::AskAiPrompt(prompt)) => Some(prompt),
            _ => None,
        }
    }

    pub(super) fn ask_ai_output_mut(&mut self) -> Option<&mut AskAiOutputState> {
        match &mut self.overlay {
            Some(Overlay::AskAiOutput(output)) => Some(output),
            _ => None,
        }
    }

    fn handle_help_overlay_key(&mut self, key: KeyEvent) {
        if closes_help_overlay(key, self.keybinds) {
            self.overlay = None;
            return;
        }

        let keybinds = self.keybinds;
        if let Some(scroll) = self.help_scroll_mut() {
            apply_scroll_key(scroll, key, HELP_OVERLAY_SCROLL_PAGE, keybinds);
        }
    }

    fn handle_help_overlay_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            if let Some(scroll) = app.help_scroll_mut() {
                scroll.scroll_by(direction, MOUSE_WHEEL_STEP);
            }
        });
    }

    fn handle_discard_confirmation_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.execute_pending_discard(),
            KeyCode::Char('y') if accepts_text_input(key) => self.execute_pending_discard(),
            KeyCode::Esc => self.overlay = None,
            KeyCode::Char('n') if accepts_text_input(key) => self.overlay = None,
            _ => {}
        }
    }

    fn handle_command_output_key(&mut self, key: KeyEvent) {
        if closes_command_output(key, self.keybinds) {
            self.overlay = None;
            return;
        }

        let keybinds = self.keybinds;
        if let Some(output) = self.command_output_mut() {
            let page = output.scroll.page();
            apply_scroll_key(&mut output.scroll, key, page, keybinds);
        }
    }

    fn handle_command_output_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            if let Some(output) = app.command_output_mut() {
                output.scroll.scroll_by(direction, MOUSE_WHEEL_STEP);
            }
        });
    }

    fn handle_ask_ai_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Enter => self.submit_ask_ai_prompt(),
            KeyCode::Backspace => {
                if let Some(prompt) = self.ask_ai_prompt_mut() {
                    prompt.input.pop();
                }
            }
            KeyCode::Char(value) if accepts_text_input(key) => {
                if let Some(prompt) = self.ask_ai_prompt_mut() {
                    prompt.input.push(value);
                }
            }
            _ => {}
        }
    }

    fn handle_ask_ai_running_key(&mut self, key: KeyEvent) {
        if !closes_ask_ai_running(key, self.keybinds) {
            return;
        }

        if let Some(Overlay::AskAiRunning {
            cancelling,
            question: _,
            spinner_frame: _,
        }) = &mut self.overlay
            && !*cancelling
        {
            *cancelling = true;
            self.ask_ai_cancel_request = true;
        }
    }

    fn handle_ask_ai_output_key(&mut self, key: KeyEvent) {
        let keybinds = self.keybinds;
        if closes_ask_ai_output(key, keybinds) {
            self.overlay = None;
            return;
        }

        if keybinds.action_for(key) == Some(BuiltinAction::CopyFocused) {
            self.queue_ask_ai_answer_copy();
            return;
        }

        if let Some(output) = self.ask_ai_output_mut() {
            let page = output.scroll.page();
            apply_scroll_key(&mut output.scroll, key, page, keybinds);
        }
    }

    fn handle_ask_ai_output_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            if let Some(output) = app.ask_ai_output_mut() {
                output.scroll.scroll_by(direction, MOUSE_WHEEL_STEP);
            }
        });
    }
}
