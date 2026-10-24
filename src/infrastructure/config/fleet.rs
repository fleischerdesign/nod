//! Fleet declaration: the flake output or the configuration file (ADR-031).
//!
//! A fleet is declared in exactly one place. Both sources at once is an error,
//! because two declarations of one fleet are two truths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::domain::desired::{self, Desired, DesiredSource};
use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::{TomlConfigStore, TomlProvider};

/// The schema version this binary understands for the `nod` flake output.
const SCHEMA: u32 = 2;

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
            providers: resolve_relative(from_file, store.base_dir()),
        }),
    }
}

/// Resolves provider paths that come from a file against the file's directory.
///
/// A path in a configuration file is relative to that file, never to the
/// directory the process happens to run in.
fn resolve_relative(
    providers: BTreeMap<String, TomlProvider>,
    base: Option<&Path>,
) -> BTreeMap<String, TomlProvider> {
    let Some(base) = base else {
        return providers;
    };

    providers
        .into_iter()
        .map(|(name, mut provider)| {
            if provider.command.is_relative() {
                provider.command = base.join(&provider.command);
            }
            if let Some(DesiredSource::Path(path)) = &mut provider.desired {
                if path.is_relative() {
                    *path = base.join(&*path);
                }
            }
            (name, provider)
        })
        .collect()
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

    if parsed.schema > SCHEMA {
        return Err(NodError::config(format!(
            "the `nod` flake output declares schema {}, this nod understands {SCHEMA}",
            parsed.schema
        )));
    }
    // An inline declaration is schema 2; a schema-1 output predates it.
    if parsed.schema < 2 {
        for (name, provider) in &parsed.providers {
            if matches!(provider.desired, Some(DesiredSource::Inline(_))) {
                return Err(NodError::config(format!(
                    "provider `{name}` declares an inline desired state, which requires the `nod` output schema 2"
                )));
            }
        }
    }

    Ok(Some(parsed))
}

/// Resolves a provider's declaration: inline, or read and parsed from a file.
pub fn resolve_desired(source: &DesiredSource) -> Result<Desired, NodError> {
    match source {
        DesiredSource::Inline(desired) => Ok(desired.clone()),
        DesiredSource::Path(path) => {
            let raw = std::fs::read_to_string(path).map_err(|error| {
                NodError::config(format!(
                    "cannot read desired state {}: {error}",
                    path.display()
                ))
            })?;
            desired::parse(&raw)
        }
    }
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
    fn an_inline_desired_state_is_a_value() {
        let raw =
            r#"{"schema": 2, "providers": {"p": {"command": "/x", "desired": {"resources": []}}}}"#;
        let parsed: FlakeFleet = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            parsed.providers.get("p").unwrap().desired,
            Some(DesiredSource::Inline(_))
        ));
    }

    #[test]
    fn an_unknown_schema_is_rejected() {
        let raw = r#"{"schema": 99, "providers": {}}"#;
        let parsed: FlakeFleet = serde_json::from_str(raw).unwrap();
        assert_ne!(parsed.schema, SCHEMA);
    }

    fn provider() -> TomlProvider {
        TomlProvider {
            command: PathBuf::from("bin/provider"),
            settings: None,
            desired: Some(DesiredSource::Path(PathBuf::from("desired.json"))),
            apply: None,
            interval_secs: None,
            order: None,
            policy: None,
            env: Vec::new(),
            timeout_secs: None,
        }
    }

    #[test]
    fn file_paths_resolve_against_the_file_directory() {
        let mut providers = BTreeMap::new();
        providers.insert("example".to_string(), provider());

        let resolved = resolve_relative(providers, Some(Path::new("/etc/nod")));
        let provider = resolved.get("example").unwrap();
        assert_eq!(provider.command, PathBuf::from("/etc/nod/bin/provider"));
        assert_eq!(
            provider.desired,
            Some(DesiredSource::Path(PathBuf::from("/etc/nod/desired.json")))
        );
    }

    #[test]
    fn absolute_paths_survive_the_resolution() {
        let mut providers = BTreeMap::new();
        let mut provider = provider();
        provider.command = PathBuf::from("/nix/store/x/bin/provider");
        provider.desired = Some(DesiredSource::Path(PathBuf::from(
            "/nix/store/y/desired.json",
        )));
        providers.insert("example".to_string(), provider);

        let resolved = resolve_relative(providers, Some(Path::new("/etc/nod")));
        let provider = resolved.get("example").unwrap();
        assert_eq!(provider.command, PathBuf::from("/nix/store/x/bin/provider"));
        assert_eq!(
            provider.desired,
            Some(DesiredSource::Path(PathBuf::from(
                "/nix/store/y/desired.json"
            )))
        );
    }
}
