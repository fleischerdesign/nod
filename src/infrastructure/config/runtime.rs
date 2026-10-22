//! Resolution of operational configuration (ADR-031).
//!
//! The operational scope cascades: CLI flag > `NOD_*` environment > `[runtime]`
//! in the configuration file > built-in default. Every value keeps its source.

use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::runtime_config::{Resolved, RuntimeConfig, Source};
use crate::infrastructure::config::toml_config::TomlRuntime;

/// Flags that may override operational values for one run.
#[derive(Debug, Clone, Default)]
pub struct RuntimeCli {
    /// Flake root.
    pub flake: Option<String>,
    /// Configuration file.
    pub config_file: Option<PathBuf>,
    /// State directory.
    pub state_dir: Option<PathBuf>,
    /// Log level.
    pub log: Option<String>,
    /// Machine-readable output by default.
    pub json: Option<bool>,
}

/// Resolves the operational scope.
pub fn resolve(
    cli: &RuntimeCli,
    env: &impl Fn(&str) -> Option<String>,
    file: Option<&TomlRuntime>,
    file_base: Option<&Path>,
) -> Result<RuntimeConfig, NodError> {
    let env_flake = env("NOD_FLAKE");
    let env_config = env("NOD_CONFIG").map(PathBuf::from);
    let env_state = env("NOD_STATE_DIR").map(PathBuf::from);
    let env_log = env("NOD_LOG");
    let env_json = env("NOD_JSON")
        .map(|raw| parse_bool("NOD_JSON", &raw))
        .transpose()?;

    let flake = pick(
        cli.flake.clone(),
        env_flake,
        file.and_then(|runtime| runtime.flake.clone()),
        ".".to_string(),
    );

    // A configuration file cannot declare itself; only the CLI and the
    // environment can point at one.
    let config_file = match (&cli.config_file, &env_config) {
        (Some(path), _) => Resolved::new(Some(path.clone()), Source::Cli),
        (None, Some(path)) => Resolved::new(Some(path.clone()), Source::Env),
        (None, None) => Resolved::new(None, Source::Default),
    };

    let state_dir = with_base(
        pick(
            cli.state_dir.clone(),
            env_state,
            file.and_then(|runtime| runtime.state_dir.clone()),
            PathBuf::from("/var/lib/nod"),
        ),
        file_base,
    );

    let log = pick(
        cli.log.clone(),
        env_log,
        file.and_then(|runtime| runtime.log.clone()),
        "info".to_string(),
    );

    let json = match (cli.json, env_json, file.and_then(|runtime| runtime.json)) {
        (Some(value), _, _) => Resolved::new(value, Source::Cli),
        (None, Some(value), _) => Resolved::new(value, Source::Env),
        (None, None, Some(value)) => Resolved::new(value, Source::File),
        (None, None, None) => Resolved::new(false, Source::Default),
    };

    Ok(RuntimeConfig {
        flake,
        config_file,
        state_dir,
        log,
        json,
    })
}

/// Picks the highest tier that has a value.
fn pick<T>(cli: Option<T>, env: Option<T>, file: Option<T>, default: T) -> Resolved<T> {
    match (cli, env, file) {
        (Some(value), _, _) => Resolved::new(value, Source::Cli),
        (None, Some(value), _) => Resolved::new(value, Source::Env),
        (None, None, Some(value)) => Resolved::new(value, Source::File),
        (None, None, None) => Resolved::new(default, Source::Default),
    }
}

/// Resolves a file-sourced relative path against the file's directory.
fn with_base(value: Resolved<PathBuf>, base: Option<&Path>) -> Resolved<PathBuf> {
    if value.source == Source::File {
        if let Some(base) = base {
            if value.value.is_relative() {
                return Resolved::new(base.join(&value.value), value.source);
            }
        }
    }
    value
}

/// Parses a boolean environment value, accepting the usual spellings.
fn parse_bool(name: &str, raw: &str) -> Result<bool, NodError> {
    match raw {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(NodError::config(format!(
            "{name} must be a boolean (1/0, true/false, yes/no, on/off), got `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn the_defaults_apply_with_no_cli_env_or_file() {
        let config = resolve(&RuntimeCli::default(), &no_env, None, None).unwrap();
        assert_eq!(config.flake.value, ".");
        assert_eq!(config.state_dir.value, PathBuf::from("/var/lib/nod"));
        assert_eq!(config.log.value, "info");
        assert!(!config.json.value);
        assert_eq!(config.flake.source, Source::Default);
        assert_eq!(config.config_file.source, Source::Default);
    }

    #[test]
    fn cli_beats_env_beats_file_beats_default() {
        let file = TomlRuntime {
            flake: Some("/file".to_string()),
            state_dir: Some(PathBuf::from("/file/state")),
            log: Some("warn".to_string()),
            json: Some(false),
        };
        let cli = RuntimeCli {
            flake: Some("/cli".to_string()),
            ..RuntimeCli::default()
        };

        let config = resolve(
            &cli,
            &env(&[("NOD_FLAKE", "/env"), ("NOD_LOG", "debug")]),
            Some(&file),
            None,
        )
        .unwrap();

        assert_eq!(config.flake.value, "/cli");
        assert_eq!(config.flake.source, Source::Cli);
        assert_eq!(config.log.value, "debug");
        assert_eq!(config.log.source, Source::Env);
        assert_eq!(config.state_dir.value, PathBuf::from("/file/state"));
        assert_eq!(config.state_dir.source, Source::File);
        assert!(!config.json.value);
        assert_eq!(config.json.source, Source::File);
    }

    #[test]
    fn a_relative_state_dir_from_the_file_resolves_against_the_file() {
        let file = TomlRuntime {
            state_dir: Some(PathBuf::from("state")),
            ..TomlRuntime::default()
        };
        let config = resolve(
            &RuntimeCli::default(),
            &no_env,
            Some(&file),
            Some(Path::new("/etc/nod")),
        )
        .unwrap();
        assert_eq!(config.state_dir.value, PathBuf::from("/etc/nod/state"));
    }

    #[test]
    fn a_config_file_only_comes_from_cli_or_env() {
        let cli = RuntimeCli {
            config_file: Some(PathBuf::from("/cli.toml")),
            ..RuntimeCli::default()
        };
        let config = resolve(&cli, &env(&[("NOD_CONFIG", "/env.toml")]), None, None).unwrap();
        assert_eq!(config.config_file.value, Some(PathBuf::from("/cli.toml")));
        assert_eq!(config.config_file.source, Source::Cli);
    }

    #[test]
    fn a_bad_boolean_in_the_environment_is_an_error() {
        let error = resolve(
            &RuntimeCli::default(),
            &env(&[("NOD_JSON", "maybe")]),
            None,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("NOD_JSON"));
    }
}
