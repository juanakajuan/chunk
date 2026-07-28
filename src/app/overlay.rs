use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::text::Line;

use crate::ask_ai::{AiHarness, AiSettings, AskAiContext, AskAiRequest, AskAiResult};
use crate::custom_command::{CustomCommandBinding, CustomCommandResult};
use crate::keybind::{BuiltinAction, KeybindMap};
use crate::rows;
use crate::scroll_text::{ScrollText, VerticalDirection};
use crate::theme::Theme;

use super::keys::{
    apply_scroll_key, closes_ask_ai_output, closes_ask_ai_running, closes_command_output,
    closes_custom_command_running, closes_help_overlay, is_ctrl_c,
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
pub(super) struct HarnessPickerState {
    pub(super) settings: AiSettings,
    pub(super) field: AiPickerField,
}

impl HarnessPickerState {
    fn move_field(&mut self, direction: VerticalDirection) {
        self.field = self.field.moved(direction);
    }

    fn change_value(&mut self, direction: VerticalDirection) {
        match self.field {
            AiPickerField::Harness => {
                self.settings.harness = match direction {
                    VerticalDirection::Down => self.settings.harness.next(),
                    VerticalDirection::Up => self.settings.harness.previous(),
                };
            }
            AiPickerField::Model => self.change_model(direction),
            AiPickerField::Effort => self.change_effort(direction),
        }
    }

    fn change_model(&mut self, direction: VerticalDirection) {
        let harness = self.settings.harness;
        let model = cycle_optional_choice(
            self.settings.selected().model(),
            harness.model_choices(),
            direction,
        );
        self.settings.selected_mut().set_model(model);

        let effort_is_supported = {
            let selected = self.settings.selected();
            selected
                .effort()
                .is_none_or(|effort| harness.effort_choices(selected.model()).contains(&effort))
        };
        if !effort_is_supported {
            self.settings.selected_mut().set_effort(None);
        }
    }

    fn change_effort(&mut self, direction: VerticalDirection) {
        let harness = self.settings.harness;
        let effort = cycle_optional_choice(
            self.settings.selected().effort(),
            harness.effort_choices(self.settings.selected().model()),
            direction,
        );
        self.settings.selected_mut().set_effort(effort);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiPickerField {
    Harness,
    Model,
    Effort,
}

impl AiPickerField {
    fn moved(self, direction: VerticalDirection) -> Self {
        match (self, direction) {
            (Self::Harness, VerticalDirection::Down) => Self::Model,
            (Self::Model, VerticalDirection::Down) => Self::Effort,
            (Self::Effort, VerticalDirection::Down) => Self::Harness,
            (Self::Harness, VerticalDirection::Up) => Self::Effort,
            (Self::Model, VerticalDirection::Up) => Self::Harness,
            (Self::Effort, VerticalDirection::Up) => Self::Model,
        }
    }
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

fn apply_output_scroll_key(scroll: &mut ScrollText, key: KeyEvent, keybinds: KeybindMap) {
    let page = scroll.page();
    apply_scroll_key(scroll, key, page, keybinds);
}

fn scroll_output_by_wheel(scroll: &mut ScrollText, direction: VerticalDirection) {
    scroll.scroll_by(direction, MOUSE_WHEEL_STEP);
}

fn mark_cancelling(cancelling: &mut bool) -> bool {
    if *cancelling {
        return false;
    }

    *cancelling = true;
    true
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
    /// A custom command is running; only cancellation keys are accepted.
    CommandRunning {
        binding: CustomCommandBinding,
        spinner_frame: usize,
        cancelling: bool,
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
    /// AI harness picker. Applying a choice persists it to config.toml.
    HarnessPicker(HarnessPickerState),
}

impl App {
    pub(crate) fn help_overlay_visible(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Help { .. }))
    }

    pub(crate) fn harness_picker_visible(&self) -> bool {
        matches!(self.overlay, Some(Overlay::HarnessPicker(_)))
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

    pub(super) fn command_running(&self) -> Option<(&CustomCommandBinding, usize, bool)> {
        match &self.overlay {
            Some(Overlay::CommandRunning {
                binding,
                spinner_frame,
                cancelling,
            }) => Some((binding, *spinner_frame, *cancelling)),
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

    fn set_diff_overlay(&mut self, overlay: Overlay) {
        self.live_error = None;
        self.focus = FocusPane::Diff;
        self.text_selection.clear();
        self.overlay = Some(overlay);
    }

    pub(crate) fn set_custom_command_running(&mut self, command: &CustomCommandBinding) {
        self.set_diff_overlay(Overlay::CommandRunning {
            binding: command.clone(),
            spinner_frame: 0,
            cancelling: false,
        });
    }

    pub(crate) fn advance_custom_command_spinner(&mut self) {
        if let Some(Overlay::CommandRunning { spinner_frame, .. }) = &mut self.overlay {
            *spinner_frame = spinner_frame.wrapping_add(1);
        }
    }

    pub(crate) fn set_custom_command_result(&mut self, result: CustomCommandResult) {
        self.set_diff_overlay(Overlay::CommandOutput(CommandOutputState {
            result,
            scroll: ScrollText::default(),
        }));
    }

    pub(crate) fn set_ask_ai_running(&mut self, request: &AskAiRequest) {
        self.set_ask_ai_running_question(request.question());
    }

    pub(crate) fn set_ask_ai_running_question(&mut self, question: &str) {
        self.set_diff_overlay(Overlay::AskAiRunning {
            question: question.to_string(),
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
        self.set_diff_overlay(Overlay::AskAiOutput(AskAiOutputState {
            result,
            scroll: ScrollText::default(),
        }));
    }

    pub(super) fn handle_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay {
            Some(Overlay::Help { .. }) => self.handle_help_overlay_key(key),
            Some(Overlay::Discard(_)) => self.handle_discard_confirmation_key(key),
            Some(Overlay::CommandRunning { .. }) => self.handle_custom_command_running_key(key),
            Some(Overlay::CommandOutput(_)) => self.handle_command_output_key(key),
            Some(Overlay::AskAiPrompt(_)) => self.handle_ask_ai_prompt_key(key),
            Some(Overlay::AskAiRunning { .. }) => self.handle_ask_ai_running_key(key),
            Some(Overlay::AskAiOutput(_)) => self.handle_ask_ai_output_key(key),
            Some(Overlay::HarnessPicker(_)) => self.handle_harness_picker_key(key),
            None => {}
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
            | Some(Overlay::HarnessPicker(_))
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

    pub(super) fn open_harness_picker(&mut self) {
        self.live_error = None;
        self.overlay = Some(Overlay::HarnessPicker(HarnessPickerState {
            settings: self.ai_settings.clone(),
            field: AiPickerField::Harness,
        }));
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

    fn harness_picker_mut(&mut self) -> Option<&mut HarnessPickerState> {
        match &mut self.overlay {
            Some(Overlay::HarnessPicker(picker)) => Some(picker),
            _ => None,
        }
    }

    fn with_harness_picker(&mut self, update: impl FnOnce(&mut HarnessPickerState)) {
        if let Some(picker) = self.harness_picker_mut() {
            update(picker);
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
            apply_output_scroll_key(&mut output.scroll, key, keybinds);
        }
    }

    fn handle_command_output_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            if let Some(output) = app.command_output_mut() {
                scroll_output_by_wheel(&mut output.scroll, direction);
            }
        });
    }

    fn handle_custom_command_running_key(&mut self, key: KeyEvent) {
        if !is_ctrl_c(key) && !closes_custom_command_running(key, self.keybinds) {
            return;
        }

        if let Some(Overlay::CommandRunning {
            cancelling,
            binding: _,
            spinner_frame: _,
        }) = &mut self.overlay
            && mark_cancelling(cancelling)
        {
            self.queue_custom_command_cancel_effect();
        }
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
            && mark_cancelling(cancelling)
        {
            self.queue_ask_ai_cancel_effect();
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
            apply_output_scroll_key(&mut output.scroll, key, keybinds);
        }
    }

    fn handle_ask_ai_output_mouse(&mut self, mouse: MouseEvent) {
        self.handle_selectable_text_mouse(mouse, |app, direction| {
            if let Some(output) = app.ask_ai_output_mut() {
                scroll_output_by_wheel(&mut output.scroll, direction);
            }
        });
    }

    fn handle_harness_picker_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Enter => self.apply_selected_ai_settings(),
            KeyCode::Char(choice @ ('1' | '2' | '3')) if accepts_text_input(key) => {
                let harness = match choice {
                    '1' => AiHarness::OpenCode,
                    '2' => AiHarness::ClaudeCode,
                    '3' => AiHarness::Codex,
                    _ => unreachable!(),
                };
                self.with_harness_picker(|picker| picker.settings.harness = harness);
            }
            KeyCode::Down => {
                self.with_harness_picker(|picker| picker.move_field(VerticalDirection::Down));
            }
            KeyCode::Up => {
                self.with_harness_picker(|picker| picker.move_field(VerticalDirection::Up));
            }
            KeyCode::Right => {
                self.with_harness_picker(|picker| picker.change_value(VerticalDirection::Down));
            }
            KeyCode::Left => {
                self.with_harness_picker(|picker| picker.change_value(VerticalDirection::Up));
            }
            KeyCode::Char('l') if accepts_text_input(key) => {
                self.with_harness_picker(|picker| picker.change_value(VerticalDirection::Down));
            }
            KeyCode::Char('h') if accepts_text_input(key) => {
                self.with_harness_picker(|picker| picker.change_value(VerticalDirection::Up));
            }
            _ => match self.keybinds.action_for(key) {
                Some(BuiltinAction::MoveDown) => {
                    self.with_harness_picker(|picker| picker.move_field(VerticalDirection::Down));
                }
                Some(BuiltinAction::MoveUp) => {
                    self.with_harness_picker(|picker| picker.move_field(VerticalDirection::Up));
                }
                _ => {}
            },
        }
    }

    fn apply_selected_ai_settings(&mut self) {
        let Some(Overlay::HarnessPicker(picker)) = self.overlay.take() else {
            return;
        };
        if self.ai_settings == picker.settings {
            return;
        }

        self.ai_settings = picker.settings;
        self.live_error = None;
        self.set_live_notice(format!(
            "AI settings: {}",
            self.ai_settings.harness.display_name()
        ));
        self.queue_ai_settings_save_effect();
    }
}

fn cycle_optional_choice(
    current: Option<&str>,
    choices: &[&str],
    direction: VerticalDirection,
) -> Option<String> {
    // Position zero represents the unset value; choices occupy positions 1..=len.
    let current_position = current
        .and_then(|current| choices.iter().position(|choice| *choice == current))
        .map(|index| index + 1)
        .unwrap_or(0);
    let position_count = choices.len() + 1;
    let next_position = match direction {
        VerticalDirection::Down => (current_position + 1) % position_count,
        VerticalDirection::Up => (current_position + position_count - 1) % position_count,
    };
    match next_position {
        0 => None,
        position => Some(choices[position - 1].to_string()),
    }
}
