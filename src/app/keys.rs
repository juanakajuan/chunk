use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

pub(super) fn closes_help_overlay(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || matches!(key.code, KeyCode::Char('?') | KeyCode::Char('q') if accepts_text_input(key))
}

pub(super) fn closes_command_output(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q') if accepts_text_input(key))
}

pub(super) fn closes_ask_ai_running(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q') if accepts_text_input(key))
}

pub(super) fn closes_ask_ai_output(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q') if accepts_text_input(key))
}

pub(super) fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn apply_scroll_key(scroll: &mut ScrollText, key: KeyEvent, page: usize) -> bool {
    let Some(action) = scroll_key_action(key) else {
        return false;
    };

    apply_scroll_action(scroll, action, page);
    true
}

fn scroll_key_action(key: KeyEvent) -> Option<ScrollKeyAction> {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => Some(ScrollKeyAction::Line(VerticalDirection::Down)),
        KeyCode::Up | KeyCode::Char('k') => Some(ScrollKeyAction::Line(VerticalDirection::Up)),
        KeyCode::PageDown => Some(ScrollKeyAction::Page(VerticalDirection::Down)),
        KeyCode::PageUp => Some(ScrollKeyAction::Page(VerticalDirection::Up)),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ScrollKeyAction::Page(VerticalDirection::Down))
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ScrollKeyAction::Page(VerticalDirection::Up))
        }
        KeyCode::Home | KeyCode::Char('g') => Some(ScrollKeyAction::Top),
        KeyCode::End | KeyCode::Char('G') => Some(ScrollKeyAction::Bottom),
        _ => None,
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
