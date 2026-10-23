//! `nod config` command: the effective operational configuration (ADR-031).
//!
//! It answers "why is this value what it is" for every operational key, which
//! is the question a cascade without provenance cannot answer.

use std::path::{Path, PathBuf};

use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::runtime_config::RuntimeConfig;
use crate::infrastructure::config::runtime::{resolve, RuntimeCli};
use crate::infrastructure::config::toml_config::TomlConfigStore;

/// Prints the effective configuration and each value's source.
pub async fn execute(
    flake: Option<String>,
    config: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    log: Option<String>,
    json: bool,
) -> Result<(), NodError> {
    let cli = RuntimeCli {
        flake: flake.clone(),
        config_file: config.clone(),
        state_dir,
        log,
        json: None,
    };

    let store = match &config {
        Some(path) => TomlConfigStore::from_file(path, CliOverrides::default())?,
        None => TomlConfigStore::new(
            Path::new(flake.as_deref().unwrap_or(".")),
            CliOverrides::default(),
        )?,
    };

    let runtime = resolve(
        &cli,
        &|key| std::env::var(key).ok(),
        store.runtime(),
        store.base_dir(),
    )?;

    if json {
        let text = serde_json::to_string_pretty(&runtime).map_err(|error| {
            NodError::config(format!("cannot render the configuration: {error}"))
        })?;
        println!("{text}");
    } else {
        print_table(&runtime);
    }
    Ok(())
}

/// Renders the configuration as an aligned table.
fn print_table(config: &RuntimeConfig) {
    println!("{:<12} {:<34} SOURCE", "KEY", "VALUE");

    let config_file = config
        .config_file
        .value
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());

    println!(
        "{:<12} {:<34} {}",
        "flake",
        config.flake.value,
        config.flake.source.as_str()
    );
    println!(
        "{:<12} {:<34} {}",
        "config",
        config_file,
        config.config_file.source.as_str()
    );
    println!(
        "{:<12} {:<34} {}",
        "state-dir",
        config.state_dir.value.display(),
        config.state_dir.source.as_str()
    );
    println!(
        "{:<12} {:<34} {}",
        "log",
        config.log.value,
        config.log.source.as_str()
    );
    println!(
        "{:<12} {:<34} {}",
        "json",
        config.json.value,
        config.json.source.as_str()
    );
}
