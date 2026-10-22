//! Fleet declaration: the flake output or the configuration file (ADR-031).
//!
//! A fleet is declared in exactly one place. Both sources at once is an error,
//! because two declarations of one fleet are two truths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::{TomlConfigStore, TomlProvider};

/// The schema version this binary understands for the `nod` flake output.
const SCHEMA: u32 = 1;

/// Where the fleet came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetSource {
    /// The `nod` flake output.
    Flake,
    /// The configuration file (with its path, when one was found).
    File(Option<PathBuf>),
}

/// The provider declarations of one fleet.
#[derive(Debug, Clone)]
pub struct Fleet {
    /// Where the declaration came from.
    pub source: FleetSource,
    /// Providers by name.
    pub providers: BTreeMap<String, TomlProvider>,
}

/// The shape of the `nod` flake output.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlakeFleet {
    schema: u32,
    #[serde(default)]
    providers: BTreeMap<String, TomlProvider>,
}

/// Loads the fleet, enforcing the one-source rule.
pub async fn load(flake_path: &Path, store: &TomlConfigStore) -> Result<Fleet, NodError> {
    let from_flake = read_flake(flake_path).await?;
    let from_file: BTreeMap<String, TomlProvider> = store
        .providers()
        .map(|(name, provider)| (name.clone(), provider.clone()))
        .collect();

    match (from_flake, from_file.is_empty()) {
        (Some(_), false) => Err(NodError::config(format!(
            "providers are declared twice: in the `nod` flake output and in the configuration file under {}",
            store
                .base_dir()
                .map(|base| base.display().to_string())
                .unwrap_or_else(|| "the discovered .nod.toml".to_string())
        ))),
        (Some(flake), true) => Ok(Fleet {
            source: FleetSource::Flake,
            providers: flake.providers,
        }),
        (None, _) => Ok(Fleet {
            source: FleetSource::File(store.base_dir().map(Path::to_path_buf)),
            providers: from_file,
        }),
    }
}

/// Reads `.#nod` from the flake, or `None` when the output does not exist.
async fn read_flake(flake_path: &Path) -> Result<Option<FlakeFleet>, NodError> {
    // A directory without a flake is a file-backed fleet, not an error: the
    // non-Nix path must not require Nix to answer.
    if !flake_path.join("flake.nix").exists() {
        return Ok(None);
    }

    let reference = format!("path:{}#nod", flake_path.display());
    let evaluation = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("nix")
            .args(["eval", "--json", &reference])
            .output(),
    )
    .await;

    let output = match evaluation {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(NodError::config(format!(
                "cannot evaluate `{reference}`: {error}"
            )))
        }
        Err(_) => {
            return Err(NodError::config(format!(
                "evaluating `{reference}` timed out"
            )))
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // A flake without a `nod` output is a file-backed fleet, not an error.
        if stderr.contains("does not provide attribute") && stderr.contains("'nod'") {
            return Ok(None);
        }
        return Err(NodError::config(format!(
            "cannot evaluate `{reference}`: {}",
            stderr.trim()
        )));
    }

    let parsed: FlakeFleet = serde_json::from_slice(&output.stdout).map_err(|error| {
        NodError::config(format!(
            "invalid `nod` flake output in {reference}: {error}"
        ))
    })?;

    if parsed.schema != SCHEMA {
        return Err(NodError::config(format!(
            "the `nod` flake output declares schema {}, this nod understands {}",
            parsed.schema, SCHEMA
        )));
    }

    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flake_output_uses_the_same_field_names_as_the_file() {
        let raw = r#"{
            "schema": 1,
            "providers": {
                "cloudflare": {
                    "command": "/nix/store/x/bin/nod-provider-cloudflare",
                    "settings": {"zone": "example.com"},
                    "desired": "/nix/store/y/desired.json",
                    "apply": false,
                    "interval_secs": 300,
                    "order": "before-hosts"
                }
            }
        }"#;

        let parsed: FlakeFleet = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.schema, 1);
        let provider = parsed.providers.get("cloudflare").unwrap();
        assert_eq!(
            provider.command,
            PathBuf::from("/nix/store/x/bin/nod-provider-cloudflare")
        );
        assert_eq!(provider.interval_secs, Some(300));
    }

    #[test]
    fn an_unknown_schema_is_rejected() {
        let raw = r#"{"schema": 99, "providers": {}}"#;
        let parsed: FlakeFleet = serde_json::from_str(raw).unwrap();
        assert_ne!(parsed.schema, SCHEMA);
    }
}
