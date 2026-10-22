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
pub async fn test(
    command: &Path,
    settings_path: Option<&Path>,
    write: bool,
) -> Result<(), NodError> {
    let settings = read_settings(settings_path)?;
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

    let observed = provider.observe(&settings).await?;
    println!("observe     ok   {} resource(s)", observed.resources.len());

    let plan = provider.plan(&settings, &[], &observed.resources).await?;
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

    let applied = provider.apply(&settings, &[]).await?;
    println!("apply       ok   {} applied", applied.applied.len());

    let verified = provider.observe(&settings).await?;
    println!("verify      ok   {} resource(s)", verified.resources.len());

    if write {
        let manifest = manifest_for(command).ok_or_else(|| {
            NodError::config("write conformance needs a provider.json with a sandbox")
        })?;
        let sandbox = manifest
            .sandbox
            .ok_or_else(|| NodError::config("provider declares no sandbox; refusing to write"))?;
        write_conformance(&mut provider, &handshake, &settings, &sandbox).await?;
    }

    println!("PASS");
    Ok(())
}

/// Reads provider settings from a JSON file, or an empty object.
fn read_settings(path: Option<&Path>) -> Result<serde_json::Value, NodError> {
    match path {
        None => Ok(serde_json::Value::Object(serde_json::Map::new())),
        Some(path) => {
            let raw = std::fs::read_to_string(path).map_err(|error| {
                NodError::config(format!("cannot read settings {}: {error}", path.display()))
            })?;
            serde_json::from_str(&raw).map_err(|error| {
                NodError::config(format!("cannot parse settings {}: {error}", path.display()))
            })
        }
    }
}

/// Creates the sandbox fixture, proves idempotency, then removes exactly what
/// the fixture named.
async fn write_conformance(
    provider: &mut ProcessProvider,
    handshake: &crate::domain::provider::Handshake,
    settings: &serde_json::Value,
    sandbox: &crate::domain::provider::Sandbox,
) -> Result<(), NodError> {
    use crate::application::use_cases::reconcile::approve;
    use crate::domain::provider::{Change, ChangeKind, DesiredResource};

    if !handshake.ownership.allows_writes() {
        return Err(NodError::config(
            "provider is read-only; there is nothing to write",
        ));
    }

    let observed = provider.observe(settings).await?;
    let plan = provider
        .plan(settings, &sandbox.desired, &observed.resources)
        .await?;
    let (approved, refused) = approve(
        handshake,
        &plan.changes,
        // The sandbox owns exactly the resources it declared, and it removes
        // exactly those. The cap is the fixture size, not a fleet policy.
        &crate::domain::policy::Policy {
            protect: Vec::new(),
            max_deletions: sandbox.desired.len(),
        },
    );
    if !refused.is_empty() {
        return Err(NodError::provider(format!(
            "sandbox plan refused: {}",
            refused.join("; ")
        )));
    }

    let applied = provider.apply(settings, &approved).await?;
    if applied.applied.iter().any(|entry| !entry.ok) {
        return Err(NodError::provider("apply reported a failed change"));
    }

    // Idempotency: the same declaration must now produce an empty plan.
    let after = provider.observe(settings).await?;
    let second = provider
        .plan(settings, &sandbox.desired, &after.resources)
        .await?;
    if !second.changes.is_empty() {
        return Err(NodError::provider(format!(
            "provider is not idempotent: {} change(s) after apply",
            second.changes.len()
        )));
    }

    // Cleanup deletes exactly the fixture ids, never the whole namespace.
    let ids: Vec<&str> = sandbox
        .desired
        .iter()
        .map(|resource| resource.id.as_str())
        .collect();
    let deletes: Vec<Change> = after
        .resources
        .iter()
        .filter(|resource| ids.contains(&resource.id.as_str()) && resource.owned)
        .map(|resource| Change {
            kind: ChangeKind::Delete,
            owned: true,
            destructive: true,
            resource: DesiredResource {
                kind: resource.kind.clone(),
                id: resource.id.clone(),
                attributes: resource.attributes.clone(),
            },
        })
        .collect();

    if !deletes.is_empty() {
        let cleanup = provider.apply(settings, &deletes).await?;
        if cleanup.applied.iter().any(|entry| !entry.ok) {
            return Err(NodError::provider(
                "cleanup failed; the sandbox may hold leftovers",
            ));
        }
    }

    println!("write       ok   created, idempotent, cleaned up");
    Ok(())
}
