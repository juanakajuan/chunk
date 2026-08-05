//! User configuration loading.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::Deserialize;

use crate::ask_ai::{AiHarness, AiHarnessSettings, AiSettings};
use crate::custom_command::{CommandKey, CustomCommandBinding};
use crate::keybind::{BuiltinKey, KeybindMap, parse_action_name};
use crate::theme::ThemeName;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppConfig {
    pub(crate) theme: ThemeName,
    pub(crate) ai: AiSettings,
    pub(crate) commands: Vec<CustomCommandBinding>,
    pub(crate) keybinds: KeybindMap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    theme: Option<String>,
    harness: Option<String>,
    opencode_model: Option<String>,
    opencode_variant: Option<String>,
    claude_model: Option<String>,
    claude_effort: Option<String>,
    codex_model: Option<String>,
    codex_effort: Option<String>,
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

pub(crate) fn save_ai_settings(settings: &AiSettings) -> Result<()> {
    let Some(path) = config_path() else {
        return Err(eyre!(
            "could not determine config path: XDG_CONFIG_HOME and HOME are unset"
        ));
    };
    save_ai_settings_to_path(&path, settings)
}

fn save_ai_settings_to_path(path: &Path, settings: &AiSettings) -> Result<()> {
    let source = if path.exists() {
        fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read config {}", path.display()))?
    } else {
        String::new()
    };
    let updated = set_ai_settings_source(&source, settings);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create config directory {}", parent.display()))?;
    }
    fs::write(path, updated).wrap_err_with(|| format!("failed to write config {}", path.display()))
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
    let mut ai = AiSettings {
        harness: parse_harness(raw.harness.as_deref())?,
        ..AiSettings::default()
    };
    for (harness, model, effort) in [
        (
            AiHarness::OpenCode,
            raw.opencode_model,
            raw.opencode_variant,
        ),
        (AiHarness::ClaudeCode, raw.claude_model, raw.claude_effort),
        (AiHarness::Codex, raw.codex_model, raw.codex_effort),
    ] {
        *ai.for_harness_mut(harness) = parse_harness_settings(harness, model, effort)?;
    }
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
        ai,
        commands,
        keybinds,
    })
}

fn parse_harness(harness: Option<&str>) -> Result<AiHarness> {
    let Some(harness) = harness else {
        return Ok(AiHarness::default());
    };

    AiHarness::from_config_value(harness).ok_or_else(|| {
        eyre!(
            "invalid harness `{}`; expected one of: {}",
            harness,
            AiHarness::CONFIG_VALUES.join(", ")
        )
    })
}

fn parse_harness_settings(
    harness: AiHarness,
    model: Option<String>,
    effort: Option<String>,
) -> Result<AiHarnessSettings> {
    let model = model
        .map(|model| validate_nonempty_ai_value(harness, "model", model))
        .transpose()?;
    let effort = effort
        .map(|effort| validate_effort(harness, effort))
        .transpose()?;
    Ok(AiHarnessSettings::new(model, effort))
}

fn validate_nonempty_ai_value(harness: AiHarness, field: &str, value: String) -> Result<String> {
    if value.trim().is_empty() {
        return Err(eyre!("{} {field} cannot be empty", harness.display_name()));
    }
    Ok(value)
}

fn validate_effort(harness: AiHarness, effort: String) -> Result<String> {
    const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
    const CODEX_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];
    let expected = match harness {
        AiHarness::OpenCode => {
            return validate_nonempty_ai_value(harness, "variant", effort);
        }
        AiHarness::ClaudeCode => CLAUDE_EFFORTS,
        AiHarness::Codex => CODEX_EFFORTS,
    };
    if expected.contains(&effort.as_str()) {
        return Ok(effort);
    }

    Err(eyre!(
        "invalid {} effort `{effort}`; expected one of: {}",
        harness.display_name(),
        expected.join(", ")
    ))
}

fn set_ai_settings_source(source: &str, settings: &AiSettings) -> String {
    let replacements = [
        ("harness", Some(settings.harness.config_value())),
        ("opencode_model", settings.opencode.model()),
        ("opencode_variant", settings.opencode.effort()),
        ("claude_model", settings.claude_code.model()),
        ("claude_effort", settings.claude_code.effort()),
        ("codex_model", settings.codex.model()),
        ("codex_effort", settings.codex.effort()),
    ];
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let trailing_newline = source.ends_with('\n');
    let mut top_level_end = lines
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .unwrap_or(lines.len());

    for (key, value) in replacements {
        update_top_level_config_line(
            &mut lines,
            &mut top_level_end,
            key,
            value.map(quote_config_value),
        );
    }

    finish_config_lines(lines, trailing_newline || source.is_empty())
}

fn update_top_level_config_line(
    lines: &mut Vec<String>,
    top_level_end: &mut usize,
    key: &str,
    value: Option<String>,
) {
    let existing_index = lines[..*top_level_end]
        .iter()
        .position(|line| is_top_level_config_line(line, key));

    match (existing_index, value) {
        (Some(index), Some(value)) => lines[index] = format!("{key} = {value}"),
        (Some(index), None) => {
            lines.remove(index);
            *top_level_end -= 1;
        }
        (None, Some(value)) => {
            lines.insert(*top_level_end, format!("{key} = {value}"));
            *top_level_end += 1;
        }
        (None, None) => {}
    }
}

fn quote_config_value(value: &str) -> String {
    format!("{value:?}")
}

fn is_top_level_config_line(line: &str, expected_key: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.starts_with('#')
        && trimmed
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == expected_key)
}

fn finish_config_lines(lines: Vec<String>, trailing_newline: bool) -> String {
    let mut source = lines.join("\n");
    if trailing_newline {
        source.push('\n');
    }
    source
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
    fn parses_ai_harness() {
        let config = parse(r#"harness = "codex""#).unwrap();
        assert_eq!(config.ai.harness, AiHarness::Codex);

        let config = parse(r#"harness = "claude-code""#).unwrap();
        assert_eq!(config.ai.harness, AiHarness::ClaudeCode);

        let default_config = parse("").unwrap();
        assert_eq!(default_config.ai.harness, AiHarness::OpenCode);
    }

    #[test]
    fn parses_harness_specific_models_and_effort_names() {
        let config = parse(
            r#"
opencode_model = "opencode/gpt-5.6-sol"
opencode_variant = "max"
claude_model = "fable"
claude_effort = "xhigh"
codex_model = "gpt-5.6-terra"
codex_effort = "ultra"
"#,
        )
        .unwrap();

        assert_eq!(config.ai.opencode.model(), Some("opencode/gpt-5.6-sol"));
        assert_eq!(config.ai.opencode.effort(), Some("max"));
        assert_eq!(config.ai.claude_code.model(), Some("fable"));
        assert_eq!(config.ai.claude_code.effort(), Some("xhigh"));
        assert_eq!(config.ai.codex.model(), Some("gpt-5.6-terra"));
        assert_eq!(config.ai.codex.effort(), Some("ultra"));
    }

    #[test]
    fn rejects_invalid_theme() {
        let error = parse(r#"theme = "solarized""#).unwrap_err();

        assert!(error.to_string().contains("invalid theme `solarized`"));
        assert!(error.to_string().contains("gruvbox, github-dark"));
    }

    #[test]
    fn rejects_invalid_harness() {
        let error = parse(r#"harness = "copilot""#).unwrap_err();

        assert!(error.to_string().contains("invalid harness `copilot`"));
        assert!(error.to_string().contains("opencode, claude-code, codex"));
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

    #[test]
    fn save_ai_settings_to_path_creates_or_updates_top_level_settings() {
        let root = temp_root();
        let path = root.join("chunk/config.toml");

        save_ai_settings_to_path(&path, &AiSettings::with_harness(AiHarness::Codex)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "harness = \"codex\"\n");
        assert_eq!(load_from_path(&path).unwrap().ai.harness, AiHarness::Codex);

        fs::write(
            &path,
            r#"theme = "github-dark"

[keybinds]
quit = "Q"
"#,
        )
        .unwrap();
        save_ai_settings_to_path(&path, &AiSettings::with_harness(AiHarness::ClaudeCode)).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"theme = "github-dark"

harness = "claude-code"
[keybinds]
quit = "Q"
"#
        );
        assert_eq!(
            load_from_path(&path).unwrap().ai.harness,
            AiHarness::ClaudeCode
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_ai_settings_to_path_replaces_existing_top_level_harness_only() {
        let root = temp_root();
        let path = root.join("chunk/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"harness = "opencode" # old

[keybinds]
harness = "H"
"#,
        )
        .unwrap();

        save_ai_settings_to_path(&path, &AiSettings::with_harness(AiHarness::Codex)).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"harness = "codex"

[keybinds]
harness = "H"
"#
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_ai_settings_persists_each_harness_profile() {
        let root = temp_root();
        let path = root.join("chunk/config.toml");
        let mut settings = AiSettings::with_harness(AiHarness::Codex);
        settings
            .opencode
            .set_model(Some("opencode/claude-fable-5".to_string()));
        settings.opencode.set_effort(Some("xhigh".to_string()));
        settings.claude_code.set_model(Some("fable".to_string()));
        settings.claude_code.set_effort(Some("max".to_string()));
        settings.codex.set_model(Some("gpt-5.6-sol".to_string()));
        settings.codex.set_effort(Some("ultra".to_string()));

        save_ai_settings_to_path(&path, &settings).unwrap();

        let source = fs::read_to_string(&path).unwrap();
        assert!(source.contains(r#"opencode_model = "opencode/claude-fable-5""#));
        assert!(source.contains(r#"opencode_variant = "xhigh""#));
        assert!(source.contains(r#"claude_model = "fable""#));
        assert!(source.contains(r#"claude_effort = "max""#));
        assert!(source.contains(r#"codex_model = "gpt-5.6-sol""#));
        assert!(source.contains(r#"codex_effort = "ultra""#));
        assert_eq!(load_from_path(&path).unwrap().ai, settings);

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
