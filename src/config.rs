//! User configuration loading.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::Deserialize;

use crate::custom_command::{CommandKey, CustomCommandBinding};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppConfig {
    pub(crate) commands: Vec<CustomCommandBinding>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    commands: Vec<RawCommand>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    key: String,
    label: String,
    command: String,
}

pub(crate) fn load() -> Result<AppConfig> {
    let Some(path) = config_path() else {
        return Ok(AppConfig::default());
    };
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let source = fs::read_to_string(&path)
        .wrap_err_with(|| format!("failed to read config {}", path.display()))?;
    parse(&source).wrap_err_with(|| format!("invalid config {}", path.display()))
}

fn config_path() -> Option<PathBuf> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(config_home).join("chunk/config.toml"));
    }

    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config/chunk/config.toml"))
}

fn parse(source: &str) -> Result<AppConfig> {
    let raw: RawConfig = toml::from_str(source)?;
    let mut keys = HashSet::new();
    let mut commands = Vec::with_capacity(raw.commands.len());

    for raw_command in raw.commands {
        let key = CommandKey::parse(&raw_command.key)?;
        validate_command(&raw_command, key, &mut keys)?;
        commands.push(CustomCommandBinding::new(
            key,
            raw_command.label,
            raw_command.command,
        ));
    }

    Ok(AppConfig { commands })
}

fn validate_command(
    command: &RawCommand,
    key: CommandKey,
    keys: &mut HashSet<CommandKey>,
) -> Result<()> {
    if key.conflicts_with_builtin() {
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
}
