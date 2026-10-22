//! `nod provider` commands: inspect and test provider executables (ADR-030).
//!
//! These commands never touch a fleet. They answer what a provider is, what it
//! can do, and whether it keeps the protocol — the questions that decide
//! whether an ecosystem can exist at all.

use std::path::Path;
use std::time::Duration;

use crate::domain::errors::NodError;
use crate::domain::provider::Manifest;
use crate::infrastructure::provider::process::ProcessProvider;

/// Timeout for provider operations during inspection and conformance.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Lists providers by their manifests, without executing them.
///
/// Listing what is installed must not require running it: a manifest is
/// metadata, and only `inspect` or `test` execute the provider.
pub async fn list(dir: &Path) -> Result<(), NodError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| NodError::config(format!("cannot read {}: {error}", dir.display())))?;

    let mut manifests: Vec<(std::path::PathBuf, Manifest)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("provider.json");
        if path.is_file() {
            manifests.push((path.clone(), Manifest::read(&path)?));
        }
    }
    manifests.sort_by(|left, right| left.0.cmp(&right.0));

    if manifests.is_empty() {
        println!("no provider manifests under {}", dir.display());
        return Ok(());
    }

    println!(
        "{:<16} {:<10} {:<9} {:<12} CAPABILITIES",
        "NAME", "VERSION", "PROTOCOL", "OWNERSHIP"
    );
    for (_, manifest) in manifests {
        println!(
            "{:<16} {:<10} {:<9} {:<12} {}",
            manifest.name,
            manifest.version,
            manifest.protocol,
            format!("{:?}", manifest.ownership).to_lowercase(),
            manifest.capabilities.join(",")
        );
    }
    Ok(())
}

/// Reads `provider.json` next to `command`, when present.
fn manifest_for(command: &Path) -> Option<Manifest> {
    let path = command.parent()?.join("provider.json");
    if !path.is_file() {
        return None;
    }
    Manifest::read(&path).ok()
}

/// Prints the provider's handshake.
pub async fn inspect(command: &Path) -> Result<(), NodError> {
    let (_provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT).await?;
    let text = serde_json::to_string_pretty(&handshake)
        .map_err(|error| NodError::provider(format!("cannot render handshake: {error}")))?;
    println!("{text}");
    Ok(())
}

/// Prints the provider's settings schema.
pub async fn schema(command: &Path) -> Result<(), NodError> {
    let (_provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT).await?;
    let text = serde_json::to_string_pretty(&handshake.settings_schema)
        .map_err(|error| NodError::provider(format!("cannot render schema: {error}")))?;
    println!("{text}");
    Ok(())
}

/// Runs the protocol conformance checks.
///
/// Nothing here proves the provider talks to its API correctly - that needs the
/// provider's own fake API. It proves the properties the core depends on:
/// handshake, a valid observe, a plan that obeys ownership, and an apply that
/// accepts an empty change set.
pub async fn test(command: &Path) -> Result<(), NodError> {
    let (mut provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT).await?;

    println!(
        "handshake   ok   {} {} (protocol {})",
        handshake.provider, handshake.version, handshake.protocol
    );

    match manifest_for(command) {
        Some(manifest) => {
            if manifest.name != handshake.provider {
                return Err(NodError::provider(format!(
                    "manifest names `{}` but the handshake says `{}`",
                    manifest.name, handshake.provider
                )));
            }
            if manifest.protocol != handshake.protocol {
                return Err(NodError::provider(format!(
                    "manifest declares protocol {} but the provider speaks {}",
                    manifest.protocol, handshake.protocol
                )));
            }
            println!("manifest    ok   matches the handshake");
        }
        None => println!("manifest    --   none next to the binary"),
    }
    println!(
        "ownership   ok   {}{}",
        format!("{:?}", handshake.ownership).to_lowercase(),
        if handshake.ownership.allows_writes() {
            ""
        } else {
            " (read-only)"
        }
    );

    let config = serde_json::Value::Object(serde_json::Map::new());
    let observed = provider.observe(&config).await?;
    println!("observe     ok   {} resource(s)", observed.resources.len());

    let plan = provider.plan(&config, &[], &observed.resources).await?;
    println!("plan        ok   {} change(s)", plan.changes.len());

    if let Some(change) = plan
        .changes
        .iter()
        .find(|change| change.destructive && !change.owned)
    {
        return Err(NodError::provider(format!(
            "provider proposed a destructive change on an unowned resource: {}",
            change.resource.id
        )));
    }

    let applied = provider.apply(&config, &[]).await?;
    println!("apply       ok   {} applied", applied.applied.len());

    let verified = provider.observe(&config).await?;
    println!("verify      ok   {} resource(s)", verified.resources.len());

    println!("PASS");
    Ok(())
}
