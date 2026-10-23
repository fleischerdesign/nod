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

/// Preflight for `settings_schema`: shape and required keys.
///
/// This is deliberately not a JSON Schema validator. The provider remains the
/// authority on types and semantics; the core catches the common failure — a
/// malformed schema or a missing required setting — before the provider starts.
fn check_settings(
    schema: &serde_json::Value,
    settings: &serde_json::Value,
) -> Result<(), NodError> {
    let Some(object) = schema.as_object() else {
        return Err(NodError::provider("settings_schema must be a JSON object"));
    };
    if object.is_empty() {
        return Ok(());
    }
    if let Some(kind) = object.get("type") {
        if kind.as_str() != Some("object") {
            return Err(NodError::provider(
                "settings_schema must describe an object (`type: object`)",
            ));
        }
    }
    let Some(required) = object.get("required") else {
        return Ok(());
    };
    let Some(required) = required.as_array() else {
        return Err(NodError::provider(
            "settings_schema.required must be an array",
        ));
    };
    let Some(settings) = settings.as_object() else {
        return Err(NodError::config("settings must be a JSON object"));
    };
    for key in required {
        let Some(key) = key.as_str() else {
            return Err(NodError::provider(
                "settings_schema.required must contain strings",
            ));
        };
        if !settings.contains_key(key) {
            return Err(NodError::config(format!(
                "settings: required key `{key}` is missing"
            )));
        }
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
    let (_provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT, &[]).await?;
    let text = serde_json::to_string_pretty(&handshake)
        .map_err(|error| NodError::provider(format!("cannot render handshake: {error}")))?;
    println!("{text}");
    Ok(())
}

/// Prints the provider's settings schema.
pub async fn schema(command: &Path) -> Result<(), NodError> {
    let (_provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT, &[]).await?;
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
    env: &[String],
) -> Result<(), NodError> {
    let settings = read_settings(settings_path)?;
    let (mut provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT, env).await?;

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
            for operation in &manifest.operations {
                if !handshake.operations.contains(operation) {
                    return Err(NodError::provider(format!(
                        "manifest declares operation `{operation}` but the provider does not offer it"
                    )));
                }
            }
            for permission in &manifest.required_permissions {
                if !handshake.required_permissions.contains(permission) {
                    return Err(NodError::provider(format!(
                        "manifest requires permission `{permission}` but the provider does not declare it"
                    )));
                }
            }
            println!("manifest    ok   matches the handshake");
        }
        None => println!("manifest    --   none next to the binary"),
    }

    check_settings(&handshake.settings_schema, &settings)?;
    println!("settings    ok   schema shape and required keys");

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
    assert_no_secret(&observed, env)?;
    println!("observe     ok   {} resource(s)", observed.resources.len());

    let plan = provider.plan(&settings, &[], &observed.resources).await?;
    assert_no_secret(&plan, env)?;
    println!("plan        ok   {} change(s)", plan.changes.len());

    // A declaration that names capabilities must stay inside them — checked
    // here as well, so `provider test` catches it before a real run does.
    crate::application::use_cases::reconcile::check_capabilities(
        &handshake,
        &observed.resources,
        &plan.changes,
    )?;
    println!("capabilities ok  stays inside the declaration");

    // An optional operation that is declared must exist: an empty call proves
    // the provider answers and writes nothing.
    if handshake
        .operations
        .iter()
        .any(|operation| operation == "adopt")
    {
        let adopted = provider.adopt(&settings, &[]).await?;
        assert_no_secret(&adopted, env)?;
        if !adopted.adopted.is_empty() {
            return Err(NodError::provider(
                "a provider adopted resources when asked to adopt none",
            ));
        }
        println!("adopt       ok   answers an empty request");
    } else {
        println!("adopt       --   not offered");
    }

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
    assert_no_secret(&applied, env)?;
    println!("apply       ok   {} applied", applied.applied.len());

    let verified = provider.observe(&settings).await?;
    assert_no_secret(&verified, env)?;
    println!("verify      ok   {} resource(s)", verified.resources.len());

    if write {
        let manifest = manifest_for(command).ok_or_else(|| {
            NodError::config("write conformance needs a provider.json with a sandbox")
        })?;
        let sandbox = manifest
            .sandbox
            .ok_or_else(|| NodError::config("provider declares no sandbox; refusing to write"))?;
        write_conformance(&mut provider, &handshake, &settings, &sandbox, env).await?;
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
    env: &[String],
) -> Result<(), NodError> {
    use crate::application::use_cases::reconcile::approve;
    use crate::domain::provider::{Change, ChangeKind, DesiredResource};

    if !handshake.ownership.allows_writes() {
        return Err(NodError::config(
            "provider is read-only; there is nothing to write",
        ));
    }

    let observed = provider.observe(settings).await?;
    assert_no_secret(&observed, env)?;
    let plan = provider
        .plan(settings, &sandbox.desired, &observed.resources)
        .await?;
    assert_no_secret(&plan, env)?;
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
    assert_no_secret(&applied, env)?;
    if applied.applied.iter().any(|entry| !entry.ok) {
        return Err(NodError::provider("apply reported a failed change"));
    }

    // Idempotency: the same declaration must now produce an empty plan.
    let after = provider.observe(settings).await?;
    assert_no_secret(&after, env)?;
    // Apply must have written what it reported; an empty plan alone proves
    // nothing if observe never saw the fixture.
    assert_fixture_observed(sandbox, &after.resources)?;
    let second = provider
        .plan(settings, &sandbox.desired, &after.resources)
        .await?;
    assert_no_secret(&second, env)?;
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
        assert_no_secret(&cleanup, env)?;
        if cleanup.applied.iter().any(|entry| !entry.ok) {
            return Err(NodError::provider(
                "cleanup failed; the sandbox may hold leftovers",
            ));
        }
    }

    println!("write       ok   created, idempotent, cleaned up");
    Ok(())
}

/// Refuses a provider response that echoes a secret from its environment.
///
/// `env` names the variables the core passes to the provider; their values are
/// secrets by declaration. A provider that prints one in a response has leaked
/// it, and the test must fail before the value reaches a log.
fn assert_no_secret<T: serde::Serialize>(value: &T, env: &[String]) -> Result<(), NodError> {
    let secrets: Vec<(&str, String)> = env
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (name.as_str(), value))
        })
        .collect();
    scan_for_secrets(value, &secrets)
}

/// The testable core of [`assert_no_secret`]: scans a serialized response for
/// each named secret.
fn scan_for_secrets<T: serde::Serialize>(
    value: &T,
    secrets: &[(&str, String)],
) -> Result<(), NodError> {
    let text = serde_json::to_string(value)
        .map_err(|error| NodError::provider(format!("cannot scan a response: {error}")))?;
    for (name, secret) in secrets {
        if text.contains(secret.as_str()) {
            return Err(NodError::provider(format!(
                "provider leaked the value of {name} in its response"
            )));
        }
    }
    Ok(())
}

/// Proves that `apply` did what it said: every sandbox resource is visible in
/// the next observation.
///
/// Without this check a provider can report `ok` for a create, never write it,
/// and still pass the idempotency test with an empty plan.
fn assert_fixture_observed(
    sandbox: &crate::domain::provider::Sandbox,
    observed: &[crate::domain::provider::ObservedResource],
) -> Result<(), NodError> {
    for desired in &sandbox.desired {
        let present = observed
            .iter()
            .any(|resource| resource.kind == desired.kind && resource.id == desired.id);
        if !present {
            return Err(NodError::provider(format!(
                "apply reported success but {}:{} is not observed",
                desired.kind, desired.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_schema_is_not_checked() {
        assert!(check_settings(&json!({}), &json!({})).is_ok());
    }

    #[test]
    fn a_required_key_must_be_present() {
        let schema = json!({ "type": "object", "required": ["state"] });
        assert!(check_settings(&schema, &json!({})).is_err());
        assert!(check_settings(&schema, &json!({ "state": "/tmp/state.json" })).is_ok());
    }

    #[test]
    fn a_schema_must_be_an_object() {
        assert!(check_settings(&json!([]), &json!({})).is_err());
    }

    #[test]
    fn a_schema_must_describe_an_object() {
        assert!(check_settings(&json!({ "type": "string" }), &json!({})).is_err());
    }

    #[test]
    fn required_must_be_an_array() {
        let schema = json!({ "type": "object", "required": "state" });
        assert!(check_settings(&schema, &json!({})).is_err());
    }

    #[test]
    fn settings_must_be_an_object() {
        let schema = json!({ "type": "object", "required": ["state"] });
        assert!(check_settings(&schema, &json!([])).is_err());
    }

    #[test]
    fn a_secret_in_a_response_is_rejected() {
        let response = json!({"resources": [{"attributes": {"token": "super-secret"}}]});
        let secrets = vec![("AUTHENTIK_TOKEN", "super-secret".to_string())];
        let error = scan_for_secrets(&response, &secrets).unwrap_err();
        assert!(error.to_string().contains("AUTHENTIK_TOKEN"));
    }

    #[test]
    fn a_response_without_the_secret_passes() {
        let response = json!({"resources": []});
        let secrets = vec![("AUTHENTIK_TOKEN", "super-secret".to_string())];
        assert!(scan_for_secrets(&response, &secrets).is_ok());
    }

    #[test]
    fn a_sandbox_fixture_must_be_observed_after_apply() {
        let sandbox = crate::domain::provider::Sandbox {
            desired: vec![crate::domain::provider::DesiredResource {
                kind: "authentik.group".to_string(),
                id: "nod-conformance".to_string(),
                attributes: json!({}),
            }],
        };

        assert!(assert_fixture_observed(&sandbox, &[]).is_err());

        let observed = vec![crate::domain::provider::ObservedResource {
            kind: "authentik.group".to_string(),
            id: "nod-conformance".to_string(),
            attributes: json!({}),
            owned: true,
        }];
        assert!(assert_fixture_observed(&sandbox, &observed).is_ok());
    }
}
