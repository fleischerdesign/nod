//! `nod provider status`: a read-only summary of every declared provider.
//!
//! It answers the onboarding question the reconciler's own status cannot:
//! which declared providers are converged **and owned**, and what would change
//! if they were applied. It never applies and never records a plan; it is a
//! status command, not a cycle.

use std::path::Path;
use std::time::Duration;

use crate::application::use_cases::reconcile::{self, ReconcileOptions};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::{TomlConfigStore, TomlProvider};

/// One provider's summary, or why it could not be summarised.
struct Entry {
    name: String,
    version: String,
    observed: usize,
    unowned: usize,
    desired: usize,
    planned: usize,
    refused: usize,
    verified: usize,
    note: Option<String>,
    error: Option<String>,
}

impl Entry {
    fn failed(name: &str, error: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            version: String::new(),
            observed: 0,
            unowned: 0,
            desired: 0,
            planned: 0,
            refused: 0,
            verified: 0,
            note: None,
            error: Some(error.into()),
        }
    }

    fn skipped(name: &str, note: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            version: String::new(),
            observed: 0,
            unowned: 0,
            desired: 0,
            planned: 0,
            refused: 0,
            verified: 0,
            note: Some(note.into()),
            error: None,
        }
    }
}

/// Summarises every declared provider, or one by name.
pub async fn execute(
    provider: Option<&str>,
    json: bool,
    flake: &str,
    config: Option<&Path>,
) -> Result<(), NodError> {
    let store = match config {
        Some(path) => TomlConfigStore::from_file(path, CliOverrides::default())?,
        None => TomlConfigStore::new(Path::new(flake), CliOverrides::default())?,
    };
    let fleet = crate::infrastructure::config::fleet::load(Path::new(flake), &store).await?;

    let mut entries = Vec::new();
    for (name, declaration) in &fleet.providers {
        if provider.is_some_and(|wanted| wanted != name.as_str()) {
            continue;
        }
        entries.push(summarise(name, declaration).await);
    }

    if entries.is_empty() {
        return Err(NodError::config(match provider {
            Some(name) => format!("provider `{name}` is not declared in the fleet declaration"),
            None => "the fleet declares no providers".to_string(),
        }));
    }

    if json {
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "provider": entry.name,
                    "version": entry.version,
                    "observed": entry.observed,
                    "unowned": entry.unowned,
                    "desired": entry.desired,
                    "planned": entry.planned,
                    "refused": entry.refused,
                    "verified": entry.verified,
                    "note": entry.note,
                    "error": entry.error,
                })
            })
            .collect();
        let text = serde_json::to_string_pretty(&items)
            .map_err(|error| NodError::provider(format!("cannot render report: {error}")))?;
        println!("{text}");
        return Ok(());
    }

    println!(
        "{:<16} {:>9} {:>8} {:>8} {:>8} {:>10}",
        "PROVIDER", "OBSERVED", "UNOWNED", "PLANNED", "REFUSED", "VERIFIED"
    );
    for entry in &entries {
        if let Some(error) = &entry.error {
            println!("{:<16} ! {error}", entry.name);
            continue;
        }
        if let Some(note) = &entry.note {
            println!("{:<16} - {note}", entry.name);
            continue;
        }
        println!(
            "{:<16} {:>9} {:>8} {:>8} {:>8} {:>10}",
            entry.name,
            entry.observed,
            entry.unowned,
            entry.planned,
            entry.refused,
            format!("{}/{}", entry.verified, entry.desired)
        );
    }
    Ok(())
}

/// Runs one read-only reconcile and maps it to a summary line.
async fn summarise(name: &str, declaration: &TomlProvider) -> Entry {
    let Some(desired_path) = &declaration.desired else {
        return Entry::skipped(name, "no desired file declared; not scheduled");
    };

    let raw = match std::fs::read_to_string(desired_path) {
        Ok(raw) => raw,
        Err(error) => {
            return Entry::failed(
                name,
                format!("cannot read {}: {error}", desired_path.display()),
            )
        }
    };
    let desired = match reconcile::parse_desired(&raw) {
        Ok(desired) => desired,
        Err(error) => return Entry::failed(name, error.to_string()),
    };

    let settings = declaration
        .settings
        .clone()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let policy = declaration.policy.clone().unwrap_or_default();
    let timeout = Duration::from_secs(declaration.timeout_secs.unwrap_or(60));

    match reconcile::execute(
        &declaration.command,
        &settings,
        &desired,
        ReconcileOptions { apply: false },
        &policy,
        &declaration.env,
        timeout,
    )
    .await
    {
        Ok(report) => Entry {
            name: name.to_string(),
            version: report.version,
            observed: report.observed,
            unowned: report.unowned,
            desired: report.desired,
            planned: report.planned,
            refused: report.refused.len(),
            verified: report.verified,
            note: None,
            error: None,
        },
        Err(error) => Entry::failed(name, error.to_string()),
    }
}
