//! `nod provider adopt`: take ownership of resources that already match the
//! declaration (ADR-032).
//!
//! The provider is resolved exactly like `nod reconcile`: a declaration or an
//! ad-hoc `--command`. Nothing is inferred; `--apply` writes the marker, and
//! a conflicting resource needs `--force <id>`.

use std::path::Path;
use std::time::Duration;

use crate::application::use_cases::adopt::{self, AdoptOptions};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::TomlConfigStore;

/// Runs one adoption pass and renders the report.
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    provider: Option<&str>,
    command: Option<&Path>,
    settings_path: Option<&Path>,
    desired_path: Option<&Path>,
    force: &[String],
    apply: bool,
    env: &[String],
    json: bool,
    flake: &str,
    config: Option<&Path>,
) -> Result<(), NodError> {
    let store = match config {
        Some(path) => TomlConfigStore::from_file(path, CliOverrides::default())?,
        None => TomlConfigStore::new(Path::new(flake), CliOverrides::default())?,
    };

    let fleet = match provider {
        Some(_) => {
            Some(crate::infrastructure::config::fleet::load(Path::new(flake), &store).await?)
        }
        None => None,
    };
    let configured = match provider {
        Some(name) => Some(
            fleet
                .as_ref()
                .and_then(|fleet| fleet.providers.get(name))
                .ok_or_else(|| {
                    NodError::config(format!(
                        "provider `{name}` is not declared in the fleet declaration"
                    ))
                })?,
        ),
        None => None,
    };

    let command_path = command
        .or_else(|| configured.map(|entry| entry.command.as_path()))
        .ok_or_else(|| {
            NodError::config("pass a provider name declared in the config, or --command <path>")
        })?;

    let settings = match settings_path {
        Some(path) => read_json(path)?,
        None => configured
            .and_then(|entry| entry.settings.clone())
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
    };

    let desired_path = match desired_path {
        Some(path) => path.to_path_buf(),
        None => configured
            .and_then(|entry| entry.desired.clone())
            .ok_or_else(|| {
                NodError::config("pass --desired or declare `desired` for the provider")
            })?,
    };
    let raw = std::fs::read_to_string(&desired_path).map_err(|error| {
        NodError::config(format!(
            "cannot read desired state {}: {error}",
            desired_path.display()
        ))
    })?;
    let desired = crate::application::use_cases::reconcile::parse_desired(&raw)?;

    let environment = {
        let mut names = configured
            .map(|entry| entry.env.clone())
            .unwrap_or_default();
        for name in env {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    };
    let timeout = Duration::from_secs(
        configured
            .and_then(|entry| entry.timeout_secs)
            .unwrap_or(60),
    );

    let report = adopt::execute(
        command_path,
        &settings,
        &desired,
        AdoptOptions {
            apply,
            force: force.to_vec(),
        },
        &environment,
        timeout,
    )
    .await?;

    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "provider": report.provider,
            "version": report.version,
            "observed": report.observed,
            "unowned": report.unowned,
            "eligible": report.eligible,
            "conflicts": report
                .conflicts
                .iter()
                .map(|(id, differences)| serde_json::json!({
                    "id": id,
                    "differences": differences,
                }))
                .collect::<Vec<_>>(),
            "skipped": report.skipped,
            "adopted": report.adopted,
            "failed": report.failed,
        }))
        .map_err(|error| NodError::provider(format!("cannot render report: {error}")))?;
        println!("{text}");
        return Ok(());
    }

    println!("provider  {} {}", report.provider, report.version);
    println!("observed  {}", report.observed);
    println!("unowned   {}", report.unowned);
    println!("eligible  {}", report.eligible);
    println!("conflicts {}", report.conflicts.len());
    for (id, differences) in &report.conflicts {
        println!("          {id}: {}", differences.join("; "));
    }
    println!("skipped   {}", report.skipped);
    println!("adopted   {}", report.adopted);
    println!("failed    {}", report.failed.len());
    for reason in &report.failed {
        println!("          {reason}");
    }

    if !apply {
        println!("\ndry run: pass --apply to stamp the marker on the eligible resources");
    }
    Ok(())
}

/// Reads a JSON file, naming the path on failure.
fn read_json(path: &Path) -> Result<serde_json::Value, NodError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| NodError::config(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_str(&raw)
        .map_err(|error| NodError::config(format!("cannot parse {}: {error}", path.display())))
}
