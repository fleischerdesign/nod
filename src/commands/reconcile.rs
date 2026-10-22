//! `nod reconcile` command: one provider, one cycle (ADR-030).

use std::path::Path;

use crate::application::use_cases::reconcile::{self, ReconcileOptions};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::TomlConfigStore;

/// Runs one reconcile cycle and renders the report.
///
/// The provider is resolved from `[providers.<name>]` in the configuration; a
/// `--command` overrides the configured executable, and `--settings` overrides
/// the configured settings. Nothing is discovered by convention.
pub async fn execute(
    provider: Option<&str>,
    command: Option<&Path>,
    settings_path: Option<&Path>,
    desired_path: &Path,
    apply: bool,
    json: bool,
    flake: &str,
) -> Result<(), NodError> {
    let store = TomlConfigStore::new(Path::new(flake), CliOverrides::default())?;

    let configured = match provider {
        Some(name) => Some(store.provider(name).ok_or_else(|| {
            NodError::config(format!(
                "provider `{name}` is not declared in [providers.{name}]"
            ))
        })?),
        None => None,
    };

    let command_path = command
        .or_else(|| configured.map(|entry| entry.command.as_path()))
        .ok_or_else(|| {
            NodError::config("pass a provider name declared in the config, or --command <path>")
        })?;

    let settings = match settings_path {
        Some(path) => {
            let raw = std::fs::read_to_string(path).map_err(|error| {
                NodError::config(format!("cannot read settings {}: {error}", path.display()))
            })?;
            serde_json::from_str(&raw).map_err(|error| {
                NodError::config(format!("cannot parse settings {}: {error}", path.display()))
            })?
        }
        None => match configured.and_then(|entry| entry.settings.as_ref()) {
            Some(value) => serde_json::to_value(value).map_err(|error| {
                NodError::config(format!("cannot convert provider settings: {error}"))
            })?,
            None => serde_json::Value::Object(serde_json::Map::new()),
        },
    };

    let raw = std::fs::read_to_string(desired_path).map_err(|error| {
        NodError::config(format!(
            "cannot read desired state {}: {error}",
            desired_path.display()
        ))
    })?;
    let desired = reconcile::parse_desired(&raw)?;

    let report = reconcile::execute(
        command_path,
        &settings,
        &desired,
        ReconcileOptions { apply },
    )
    .await?;

    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "provider": report.provider,
            "observed": report.observed,
            "planned": report.planned,
            "approved": report.approved.len(),
            "refused": report.refused,
            "applied": report.applied,
            "verified": report.verified,
        }))
        .map_err(|error| NodError::provider(format!("cannot render report: {error}")))?;
        println!("{text}");
        return Ok(());
    }

    println!("provider  {}", report.provider);
    println!("observed  {}", report.observed);
    println!("planned   {}", report.planned);
    println!("approved  {}", report.approved.len());
    println!("refused   {}", report.refused.len());
    for reason in &report.refused {
        println!("          {reason}");
    }
    println!("applied   {}", report.applied);
    println!("verified  {}", report.verified);

    if !apply {
        println!("\ndry run: pass --apply to execute the approved changes");
    }
    Ok(())
}
