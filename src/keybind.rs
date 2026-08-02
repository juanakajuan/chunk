//! Configurable built-in keybindings.
//!
//! This module owns the typed map for configurable single-character built-in
//! actions. Special keys, `Ctrl-*` combos, and modal dialog controls stay fixed.

use color_eyre::eyre::{Result, eyre};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A built-in review/navigation action whose key can be configured in
/// `[keybinds]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub(crate) enum BuiltinAction {
    Quit,
    Help,
    ToggleFiles,
    Search,
    MoveDown,
    MoveUp,
    NextMatch,
    PrevMatch,
    Top,
    Bottom,
    ToggleStaging,
    Discard,
    Editor,
    AskAi,
    ExplainCode,
    UnpublishedSummary,
    CopyFocused,
    CopyFileDiff,
    ToggleReviewed,
}

struct ActionDescriptor {
    action: BuiltinAction,
    config_name: &'static str,
    default_key: char,
}

impl ActionDescriptor {
    const fn new(action: BuiltinAction, config_name: &'static str, default_key: char) -> Self {
        Self {
            action,
            config_name,
            default_key,
        }
    }
}

const BUILTIN_ACTIONS: [ActionDescriptor; 19] = [
    ActionDescriptor::new(BuiltinAction::Quit, "quit", 'q'),
    ActionDescriptor::new(BuiltinAction::Help, "help", '?'),
    ActionDescriptor::new(BuiltinAction::ToggleFiles, "toggle_files", 'f'),
    ActionDescriptor::new(BuiltinAction::Search, "search", '/'),
    ActionDescriptor::new(BuiltinAction::MoveDown, "move_down", 'j'),
    ActionDescriptor::new(BuiltinAction::MoveUp, "move_up", 'k'),
    ActionDescriptor::new(BuiltinAction::NextMatch, "next_match", 'n'),
    ActionDescriptor::new(BuiltinAction::PrevMatch, "prev_match", 'N'),
    ActionDescriptor::new(BuiltinAction::Top, "top", 'g'),
    ActionDescriptor::new(BuiltinAction::Bottom, "bottom", 'G'),
    ActionDescriptor::new(BuiltinAction::ToggleStaging, "toggle_staging", ' '),
    ActionDescriptor::new(BuiltinAction::Discard, "discard", 'd'),
    ActionDescriptor::new(BuiltinAction::Editor, "editor", 'e'),
    ActionDescriptor::new(BuiltinAction::AskAi, "ask_ai", 'a'),
    ActionDescriptor::new(BuiltinAction::ExplainCode, "explain_code", 'x'),
    ActionDescriptor::new(
        BuiltinAction::UnpublishedSummary,
        "unpublished_summary",
        'u',
    ),
    ActionDescriptor::new(BuiltinAction::CopyFocused, "copy_focused", 'y'),
    ActionDescriptor::new(BuiltinAction::CopyFileDiff, "copy_file_diff", 'Y'),
    ActionDescriptor::new(BuiltinAction::ToggleReviewed, "toggle_reviewed", 'r'),
];

impl BuiltinAction {
    pub(crate) const COUNT: usize = BUILTIN_ACTIONS.len();

    fn from_config_name(name: &str) -> Option<Self> {
        BUILTIN_ACTIONS
            .iter()
            .find(|descriptor| descriptor.config_name == name)
            .map(|descriptor| descriptor.action)
    }
}

/// A single non-control character bound to a built-in action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BuiltinKey {
    value: char,
}

impl BuiltinKey {
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        let mut chars = raw.chars();
        let Some(value) = chars.next() else {
            return Err(eyre!("keybind key cannot be empty"));
        };
        if chars.next().is_some() {
            return Err(eyre!("keybind key `{raw}` must be a single character"));
        }
        if value.is_control() {
            return Err(eyre!("keybind key cannot be a control character"));
        }
        Ok(Self { value })
    }

    const fn from_char(value: char) -> Self {
        Self { value }
    }

    #[cfg(test)]
    pub(crate) fn char(self) -> char {
        self.value
    }

    pub(crate) fn display(self) -> String {
        match self.value {
            ' ' => "Space".to_string(),
            value => value.to_string(),
        }
    }
}

/// Resolved map of built-in actions to configured keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeybindMap {
    keys: [BuiltinKey; BuiltinAction::COUNT],
}

impl Default for KeybindMap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl KeybindMap {
    pub(crate) fn defaults() -> Self {
        Self {
            keys: std::array::from_fn(|index| {
                BuiltinKey::from_char(BUILTIN_ACTIONS[index].default_key)
            }),
        }
    }

    pub(crate) fn key(self, action: BuiltinAction) -> BuiltinKey {
        self.keys[action as usize]
    }

    pub(crate) fn set(&mut self, action: BuiltinAction, key: BuiltinKey) {
        self.keys[action as usize] = key;
    }

    pub(crate) fn display(self, action: BuiltinAction) -> String {
        self.key(action).display()
    }

    /// Resolve a key event to its bound built-in action, if any.
    ///
    /// `Ctrl`/`Alt` modifiers never match: those are reserved for fixed
    /// aliases (`Ctrl-c`, `Ctrl-d`, `Ctrl-u`) or are simply no-ops.
    pub(crate) fn action_for(self, key: KeyEvent) -> Option<BuiltinAction> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let KeyCode::Char(value) = key.code else {
            return None;
        };
        BUILTIN_ACTIONS
            .iter()
            .find(|descriptor| self.key(descriptor.action).value == value)
            .map(|descriptor| descriptor.action)
    }

    /// Whether a character is reserved by any configured built-in key.
    ///
    /// Used by custom command validation to reject conflicts against the
    /// configured map rather than a static default list.
    pub(crate) fn contains_char(self, value: char) -> bool {
        self.keys.iter().any(|key| key.value == value)
    }

    /// Reject maps where two actions share the same key.
    ///
    /// Called after all config overrides are applied so that key swaps are
    /// permitted as long as the final set has no duplicates.
    pub(crate) fn validate(self) -> Result<()> {
        for (index, first) in BUILTIN_ACTIONS.iter().enumerate() {
            let first_key = self.key(first.action);
            for second in BUILTIN_ACTIONS.iter().skip(index + 1) {
                if first_key == self.key(second.action) {
                    return Err(eyre!(
                        "keybind key `{}` is bound to both `{}` and `{}`",
                        first_key.display(),
                        first.config_name,
                        second.config_name,
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Parse a `[keybinds]` action name, rejecting unknown actions with the list of
/// valid names.
pub(crate) fn parse_action_name(name: &str) -> Result<BuiltinAction> {
    BuiltinAction::from_config_name(name).ok_or_else(|| {
        eyre!(
            "unknown keybind action `{name}`; expected one of: {}",
            BUILTIN_ACTIONS
                .iter()
                .map(|descriptor| descriptor.config_name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn defaults_match_documented_keys() {
        let map = KeybindMap::defaults();
        for (action, key) in [
            (BuiltinAction::Quit, 'q'),
            (BuiltinAction::Help, '?'),
            (BuiltinAction::ToggleFiles, 'f'),
            (BuiltinAction::Search, '/'),
            (BuiltinAction::MoveDown, 'j'),
            (BuiltinAction::MoveUp, 'k'),
            (BuiltinAction::NextMatch, 'n'),
            (BuiltinAction::PrevMatch, 'N'),
            (BuiltinAction::Top, 'g'),
            (BuiltinAction::Bottom, 'G'),
            (BuiltinAction::ToggleStaging, ' '),
            (BuiltinAction::Discard, 'd'),
            (BuiltinAction::Editor, 'e'),
            (BuiltinAction::AskAi, 'a'),
            (BuiltinAction::ExplainCode, 'x'),
            (BuiltinAction::UnpublishedSummary, 'u'),
            (BuiltinAction::CopyFocused, 'y'),
            (BuiltinAction::CopyFileDiff, 'Y'),
            (BuiltinAction::ToggleReviewed, 'r'),
        ] {
            assert_eq!(map.key(action).char(), key);
        }
    }

    #[test]
    fn display_renders_space_as_word() {
        let map = KeybindMap::defaults();
        assert_eq!(map.display(BuiltinAction::ToggleStaging), "Space");
        assert_eq!(map.display(BuiltinAction::Quit), "q");
        assert_eq!(map.display(BuiltinAction::PrevMatch), "N");
    }

    #[test]
    fn action_for_resolves_bare_character_keys() {
        let map = KeybindMap::defaults();
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(BuiltinAction::Quit)
        );
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT)),
            Some(BuiltinAction::PrevMatch)
        );
    }

    #[test]
    fn action_for_ignores_control_and_alt() {
        let map = KeybindMap::defaults();
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn action_for_reflects_overrides() {
        let mut map = KeybindMap::defaults();
        map.set(BuiltinAction::Quit, BuiltinKey::from_char('Q'));
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            Some(BuiltinAction::Quit)
        );
        assert_eq!(
            map.action_for(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn contains_char_reports_reserved_characters() {
        let map = KeybindMap::defaults();
        assert!(map.contains_char('q'));
        assert!(map.contains_char('d'));
        assert!(map.contains_char(' '));
        assert!(!map.contains_char('C'));
    }

    #[test]
    fn validate_accepts_key_swaps() {
        let mut map = KeybindMap::defaults();
        map.set(BuiltinAction::Quit, BuiltinKey::from_char('d'));
        map.set(BuiltinAction::Discard, BuiltinKey::from_char('q'));
        assert!(map.validate().is_ok());
    }

    #[test]
    fn validate_rejects_shared_keys() {
        let mut map = KeybindMap::defaults();
        map.set(BuiltinAction::Quit, BuiltinKey::from_char('d'));
        let error = map.validate().unwrap_err();
        assert!(error.to_string().contains("bound to both"));
        assert!(error.to_string().contains("quit"));
        assert!(error.to_string().contains("discard"));
    }

    #[test]
    fn parse_action_name_rejects_unknown_with_valid_list() {
        let error = parse_action_name("nuke").unwrap_err();
        assert!(error.to_string().contains("unknown keybind action `nuke`"));
        assert!(error.to_string().contains("quit"));
        assert!(error.to_string().contains("toggle_reviewed"));
    }

    #[test]
    fn parse_key_rejects_empty_multi_and_control() {
        assert!(
            BuiltinKey::parse("")
                .unwrap_err()
                .to_string()
                .contains("cannot be empty")
        );
        assert!(
            BuiltinKey::parse("ab")
                .unwrap_err()
                .to_string()
                .contains("single character")
        );
        assert!(
            BuiltinKey::parse("\u{7f}")
                .unwrap_err()
                .to_string()
                .contains("control character")
        );
    }

    #[test]
    fn all_actions_have_unique_default_keys() {
        let map = KeybindMap::defaults();
        map.validate().expect("default keybinds must not collide");
    }

    #[test]
    fn action_catalog_matches_enum_order() {
        for (index, descriptor) in BUILTIN_ACTIONS.iter().enumerate() {
            assert_eq!(descriptor.action as usize, index);
        }
    }

    #[test]
    fn action_config_names_are_unique() {
        let mut names: Vec<_> = BUILTIN_ACTIONS
            .iter()
            .map(|descriptor| descriptor.config_name)
            .collect();
        names.sort();
        let initial = names.len();
        names.dedup();
        assert_eq!(names.len(), initial);
    }
}
