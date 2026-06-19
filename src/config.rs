//! User configuration loading.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::Deserialize;

use crate::custom_command::{CommandKey, CustomCommandBinding};
use crate::keybind::{BuiltinKey, KeybindMap, parse_action_name};
use crate::theme::ThemeName;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppConfig {
    pub(crate) theme: ThemeName,
    pub(crate) commands: Vec<CustomCommandBinding>,
    pub(crate) keybinds: KeybindMap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    theme: Option<String>,
    #[serde(default)]
    commands: Vec<RawCommand>,
    #[serde(default)]
    keybinds: RawKeybinds,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    key: String,
    label: String,
    command: String,
}

/// Raw `[keybinds]` table: action name -> key string. Unknown action names are
/// rejected during parsing, not by `serde`, so the error can list valid names.
#[derive(Debug, Default, Deserialize)]
struct RawKeybinds(HashMap<String, String>);

pub(crate) fn load() -> Result<AppConfig> {
    let Some(path) = config_path() else {
        return Ok(AppConfig::default());
    };
    load_from_path(&path)
}

fn load_from_path(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let source = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read config {}", path.display()))?;
    parse(&source).wrap_err_with(|| format!("invalid config {}", path.display()))
}

fn config_path() -> Option<PathBuf> {
    config_path_from_env(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

fn config_path_from_env(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(config_home) = config_home.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(config_home).join("chunk/config.toml"));
    }

    home.filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config/chunk/config.toml"))
}

fn parse(source: &str) -> Result<AppConfig> {
    let raw: RawConfig = toml::from_str(source)?;
    let theme = parse_theme(raw.theme.as_deref())?;
    let keybinds = parse_keybinds(raw.keybinds)?;
    let mut keys = HashSet::new();
    let mut commands = Vec::with_capacity(raw.commands.len());

    for raw_command in raw.commands {
        let key = CommandKey::parse(&raw_command.key)?;
        validate_command(&raw_command, key, &mut keys, &keybinds)?;
        commands.push(CustomCommandBinding::new(
            key,
            raw_command.label,
            raw_command.command,
        ));
    }

    Ok(AppConfig {
        theme,
        commands,
        keybinds,
    })
}

fn parse_theme(theme: Option<&str>) -> Result<ThemeName> {
    let Some(theme) = theme else {
        return Ok(ThemeName::default());
    };

    ThemeName::from_config_value(theme).ok_or_else(|| {
        eyre!(
            "invalid theme `{}`; expected one of: {}",
            theme,
            ThemeName::CONFIG_VALUES.join(", ")
        )
    })
}

fn parse_keybinds(raw: RawKeybinds) -> Result<KeybindMap> {
    let mut map = KeybindMap::defaults();
    for (name, raw_key) in raw.0 {
        let action = parse_action_name(&name)?;
        let key = BuiltinKey::parse(&raw_key)?;
        map.set(action, key);
    }
    map.validate()?;
    Ok(map)
}

fn validate_command(
    command: &RawCommand,
    key: CommandKey,
    keys: &mut HashSet<CommandKey>,
    keybinds: &KeybindMap,
) -> Result<()> {
    if keybinds.contains_char(key.char()) {
        return Err(eyre!(
            "custom command key `{}` conflicts with a built-in keybind",
            command.key
        ));
    }
    if !keys.insert(key) {
        return Err(eyre!("duplicate custom command key `{}`", command.key));
    }
    if command.label.trim().is_empty() {
        return Err(eyre!("custom command label cannot be empty"));
    }
    if command.command.trim().is_empty() {
        return Err(eyre!(
            "custom command `{}` shell command cannot be empty",
            command.label
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::keybind::BuiltinAction;

    use super::*;

    #[test]
    fn parses_custom_commands() {
        let config = parse(
            r#"
            [[commands]]
            key = "C"
            label = "commit and push"
            command = "ga . && com && gP"
            "#,
        )
        .unwrap();

        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].key_display(), "C");
        assert_eq!(config.commands[0].label(), "commit and push");
        assert_eq!(config.commands[0].command(), "ga . && com && gP");
    }

    #[test]
    fn parses_runtime_theme() {
        let github_dark = parse(r#"theme = "github-dark""#).unwrap();
        assert_eq!(github_dark.theme, ThemeName::GithubDark);

        let default_theme = parse("").unwrap();
        assert_eq!(default_theme.theme, ThemeName::Gruvbox);
    }

    #[test]
    fn rejects_invalid_theme() {
        let error = parse(r#"theme = "solarized""#).unwrap_err();

        assert!(error.to_string().contains("invalid theme `solarized`"));
        assert!(error.to_string().contains("gruvbox, github-dark"));
    }

    #[test]
    fn rejects_builtin_key_conflicts() {
        let error = parse(
            r#"
            [[commands]]
            key = "d"
            label = "danger"
            command = "true"
            "#,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("conflicts with a built-in keybind")
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        let error = parse(
            r#"
            [[commands]]
            key = "C"
            label = "one"
            command = "true"

            [[commands]]
            key = "C"
            label = "two"
            command = "true"
            "#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("duplicate custom command key"));
    }

    #[test]
    fn rejects_empty_command_fields() {
        let empty_label = parse(
            r#"
            [[commands]]
            key = "C"
            label = " "
            command = "true"
            "#,
        )
        .unwrap_err();
        assert!(empty_label.to_string().contains("label cannot be empty"));

        let empty_command = parse(
            r#"
            [[commands]]
            key = "C"
            label = "commit"
            command = " "
            "#,
        )
        .unwrap_err();
        assert!(
            empty_command
                .to_string()
                .contains("shell command cannot be empty")
        );
    }

    #[test]
    fn rejects_unknown_config_fields() {
        let error = parse(
            r#"
            [[commands]]
            key = "C"
            label = "commit"
            command = "true"
            unexpected = "nope"
            "#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn parses_keybind_overlays_on_top_of_defaults() {
        let config = parse(
            r#"
            [keybinds]
            quit = "Q"
            discard = "D"
            "#,
        )
        .unwrap();

        assert_eq!(key_char(&config, BuiltinAction::Quit), 'Q');
        assert_eq!(key_char(&config, BuiltinAction::Discard), 'D');
        // Untouched actions keep their defaults.
        assert_eq!(key_char(&config, BuiltinAction::Help), '?');
    }

    #[test]
    fn parses_keybind_space_token() {
        let config = parse(
            r#"
            [keybinds]
            toggle_staging = "S"
            quit = " "
            "#,
        )
        .unwrap();

        assert_eq!(key_char(&config, BuiltinAction::Quit), ' ');
        assert_eq!(key_char(&config, BuiltinAction::ToggleStaging), 'S');
    }

    #[test]
    fn rejects_unknown_keybind_action() {
        let error = parse(
            r#"
            [keybinds]
            nuke = "Q"
            "#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown keybind action `nuke`"));
        assert!(error.to_string().contains("quit"));
        assert!(error.to_string().contains("toggle_reviewed"));
    }

    #[test]
    fn rejects_invalid_keybind_keys() {
        let empty = parse(
            r#"
            [keybinds]
            quit = ""
            "#,
        )
        .unwrap_err();
        assert!(empty.to_string().contains("keybind key cannot be empty"));

        let multi = parse(
            r#"
            [keybinds]
            quit = "ab"
            "#,
        )
        .unwrap_err();
        assert!(multi.to_string().contains("single character"));

        let control = parse(
            r#"
            [keybinds]
            quit = "\u007F"
            "#,
        )
        .unwrap_err();
        assert!(control.to_string().contains("control character"));
    }

    #[test]
    fn rejects_duplicate_keybind_keys() {
        let error = parse(
            r#"
            [keybinds]
            quit = "d"
            "#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("bound to both"));
        assert!(error.to_string().contains("quit"));
        assert!(error.to_string().contains("discard"));
    }

    #[test]
    fn accepts_keybind_swaps() {
        let config = parse(
            r#"
            [keybinds]
            quit = "d"
            discard = "q"
            "#,
        )
        .unwrap();

        assert_eq!(key_char(&config, BuiltinAction::Quit), 'd');
        assert_eq!(key_char(&config, BuiltinAction::Discard), 'q');
    }

    #[test]
    fn custom_command_conflicts_with_configured_builtin_not_only_defaults() {
        // `quit` is remapped to `Q`, freeing `q` for custom commands.
        let freed = parse(
            r#"
            [keybinds]
            quit = "Q"

            [[commands]]
            key = "q"
            label = "quick"
            command = "true"
            "#,
        )
        .unwrap();
        assert_eq!(freed.commands.len(), 1);

        // `quit` is remapped to `Q`, so `Q` is now reserved for a built-in.
        let reserved = parse(
            r#"
            [keybinds]
            quit = "Q"

            [[commands]]
            key = "Q"
            label = "quick"
            command = "true"
            "#,
        )
        .unwrap_err();
        assert!(
            reserved
                .to_string()
                .contains("conflicts with a built-in keybind")
        );
    }

    #[test]
    fn config_path_prefers_xdg_config_home_over_home() {
        assert_eq!(
            config_path_from_env(
                Some(OsString::from("/xdg")),
                Some(OsString::from("/home/user"))
            ),
            Some(PathBuf::from("/xdg/chunk/config.toml"))
        );
        assert_eq!(
            config_path_from_env(None, Some(OsString::from("/home/user"))),
            Some(PathBuf::from("/home/user/.config/chunk/config.toml"))
        );
        assert_eq!(
            config_path_from_env(Some(OsString::from("")), Some(OsString::from(""))),
            None
        );
    }

    #[test]
    fn load_from_path_returns_default_when_config_is_missing() {
        let root = temp_root();
        let path = root.join("chunk/config.toml");

        assert_eq!(load_from_path(&path).unwrap(), AppConfig::default());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_from_path_reads_config_and_wraps_invalid_errors() {
        let root = temp_root();
        let path = root.join("chunk/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"
            [[commands]]
            key = "C"
            label = "commit"
            command = "true"
            "#,
        )
        .unwrap();

        let config = load_from_path(&path).unwrap();
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].label(), "commit");

        fs::write(&path, "invalid =").unwrap();
        let error = load_from_path(&path).unwrap_err();
        assert!(error.to_string().contains("invalid config"));
        assert!(format!("{error:?}").contains(path.to_str().unwrap()));

        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root() -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = env::temp_dir().join(format!("chunk-config-test-{now}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn key_char(config: &AppConfig, action: BuiltinAction) -> char {
        config.keybinds.key(action).char()
    }
}
