use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keybind::{BuiltinAction, KeybindMap};
use crate::scroll_text::{ScrollText, VerticalDirection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollKeyAction {
    Line(VerticalDirection),
    Page(VerticalDirection),
    Top,
    Bottom,
}

pub(crate) fn accepts_text_input(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

pub(super) fn closes_help_overlay(key: KeyEvent, keybinds: KeybindMap) -> bool {
    closes_with_actions(key, keybinds, &[BuiltinAction::Help, BuiltinAction::Quit])
}

pub(super) fn closes_command_output(key: KeyEvent, keybinds: KeybindMap) -> bool {
    closes_with_quit(key, keybinds)
}

pub(super) fn closes_custom_command_running(key: KeyEvent, keybinds: KeybindMap) -> bool {
    closes_with_quit(key, keybinds)
}

pub(super) fn closes_ask_ai_running(key: KeyEvent, keybinds: KeybindMap) -> bool {
    closes_with_quit(key, keybinds)
}

pub(super) fn closes_ask_ai_output(key: KeyEvent, keybinds: KeybindMap) -> bool {
    closes_with_quit(key, keybinds)
}

fn closes_with_quit(key: KeyEvent, keybinds: KeybindMap) -> bool {
    closes_with_actions(key, keybinds, &[BuiltinAction::Quit])
}

fn closes_with_actions(key: KeyEvent, keybinds: KeybindMap, actions: &[BuiltinAction]) -> bool {
    key.code == KeyCode::Esc
        || keybinds
            .action_for(key)
            .is_some_and(|action| actions.contains(&action))
}

pub(super) fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn apply_scroll_key(
    scroll: &mut ScrollText,
    key: KeyEvent,
    page: usize,
    keybinds: KeybindMap,
) -> bool {
    let Some(action) = scroll_key_action(key, keybinds) else {
        return false;
    };

    apply_scroll_action(scroll, action, page);
    true
}

fn scroll_key_action(key: KeyEvent, keybinds: KeybindMap) -> Option<ScrollKeyAction> {
    match key.code {
        KeyCode::Down => Some(ScrollKeyAction::Line(VerticalDirection::Down)),
        KeyCode::Up => Some(ScrollKeyAction::Line(VerticalDirection::Up)),
        KeyCode::PageDown => Some(ScrollKeyAction::Page(VerticalDirection::Down)),
        KeyCode::PageUp => Some(ScrollKeyAction::Page(VerticalDirection::Up)),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ScrollKeyAction::Page(VerticalDirection::Down))
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ScrollKeyAction::Page(VerticalDirection::Up))
        }
        KeyCode::Home => Some(ScrollKeyAction::Top),
        KeyCode::End => Some(ScrollKeyAction::Bottom),
        _ => match keybinds.action_for(key) {
            Some(BuiltinAction::MoveDown) => Some(ScrollKeyAction::Line(VerticalDirection::Down)),
            Some(BuiltinAction::MoveUp) => Some(ScrollKeyAction::Line(VerticalDirection::Up)),
            Some(BuiltinAction::Top) => Some(ScrollKeyAction::Top),
            Some(BuiltinAction::Bottom) => Some(ScrollKeyAction::Bottom),
            _ => None,
        },
    }
}

fn apply_scroll_action(scroll: &mut ScrollText, action: ScrollKeyAction, page: usize) {
    match action {
        ScrollKeyAction::Line(direction) => scroll.scroll_by(direction, 1),
        ScrollKeyAction::Page(direction) => scroll.scroll_by(direction, page),
        ScrollKeyAction::Top => scroll.scroll_to_top(),
        ScrollKeyAction::Bottom => scroll.scroll_to_bottom(),
    }
}
