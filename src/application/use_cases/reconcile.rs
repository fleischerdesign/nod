//! `nod reconcile`: observe, plan, approve, apply, verify (ADR-030).
//!
//! The provider describes the world; this use case decides what happens to it.
//! The policy is deliberately small and lives here, not in the provider:
//! a read-only provider may not change anything, and a destructive change on an
//! unowned resource is refused no matter who proposed it.

use std::path::Path;
use std::time::Duration;

use crate::domain::errors::NodError;
use crate::domain::provider::{Change, ChangeKind, DesiredResource, Handshake, ObservedResource};
use crate::infrastructure::provider::process::ProcessProvider;

/// Timeout for one provider operation.
const OP_TIMEOUT: Duration = Duration::from_secs(60);

/// What a reconcile run should do.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReconcileOptions {
    /// Apply the approved changes instead of only showing them.
    pub apply: bool,
}

/// What one reconcile run produced.
#[derive(Debug, Clone)]
pub struct ReconcileReport {
    /// Provider name and version.
    pub provider: String,
    /// Resources the provider reported.
    pub observed: usize,
    /// Changes the provider proposed.
    pub planned: usize,
    /// Changes the policy approved.
    pub approved: Vec<Change>,
    /// Changes the policy refused, with the reason.
    pub refused: Vec<String>,
    /// Results of applying, when `--apply` was set.
    pub applied: usize,
    /// Resources observed after applying (or after the dry run).
    pub verified: usize,
}

/// Decides which proposed changes may run.
///
/// Two rules, both non-negotiable:
/// 1. A read-only provider changes nothing.
/// 2. A destructive change on a resource the fleet does not own is refused.
pub fn approve(handshake: &Handshake, changes: &[Change]) -> (Vec<Change>, Vec<String>) {
    let mut approved = Vec::new();
    let mut refused = Vec::new();

    for change in changes {
        let label = format!("{:?} {}", change.kind, change.resource.id);

        if !handshake.ownership.allows_writes() {
            refused.push(format!("{label}: provider is read-only"));
            continue;
        }
        if change.destructive && !change.owned {
            refused.push(format!("{label}: unowned resource"));
            continue;
        }
        if change.kind == ChangeKind::Delete && !change.owned {
            refused.push(format!("{label}: unowned resource"));
            continue;
        }

        approved.push(change.clone());
    }

    (approved, refused)
}

/// Runs one reconcile cycle against one provider.
pub async fn execute(
    command: &Path,
    settings: &serde_json::Value,
    desired: &[DesiredResource],
    options: ReconcileOptions,
) -> Result<ReconcileReport, NodError> {
    let (mut provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT).await?;

    let observed = provider.observe(settings).await?;
    let plan = provider.plan(desired, &observed.resources).await?;
    let (approved, refused) = approve(&handshake, &plan.changes);

    let mut applied = 0;
    if options.apply && !approved.is_empty() {
        let result = provider.apply(&approved).await?;
        applied = result.applied.iter().filter(|entry| entry.ok).count();
    }

    // Verification is a second observation, never "the API said 200".
    let verified = provider.observe(settings).await?;

    Ok(ReconcileReport {
        provider: format!("{} {}", handshake.provider, handshake.version),
        observed: observed.resources.len(),
        planned: plan.changes.len(),
        approved,
        refused,
        applied,
        verified: verified.resources.len(),
    })
}

/// Parses a desired-state document: either a bare array or `{ "resources": [...] }`.
pub fn parse_desired(raw: &str) -> Result<Vec<DesiredResource>, NodError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| NodError::config(format!("cannot parse desired state: {error}")))?;

    let array = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(mut map) => match map.remove("resources") {
            Some(serde_json::Value::Array(items)) => items,
            _ => {
                return Err(NodError::config(
                    "desired state object must contain a `resources` array",
                ))
            }
        },
        _ => {
            return Err(NodError::config(
                "desired state must be an array or an object with `resources`",
            ))
        }
    };

    array
        .into_iter()
        .map(|item| {
            serde_json::from_value(item)
                .map_err(|error| NodError::config(format!("invalid desired resource: {error}")))
        })
        .collect()
}

/// Reports the observed resources that the declaration no longer contains.
pub fn missing_from_desired(
    desired: &[DesiredResource],
    observed: &[ObservedResource],
) -> Vec<ObservedResource> {
    observed
        .iter()
        .filter(|resource| {
            !desired
                .iter()
                .any(|want| want.kind == resource.kind && want.id == resource.id)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{DesiredResource, OwnershipMode};

    fn handshake(ownership: OwnershipMode) -> Handshake {
        Handshake {
            provider: "fake".to_string(),
            version: "0.1.0".to_string(),
            protocol: 1,
            capabilities: Vec::new(),
            ownership,
            settings_schema: serde_json::json!({}),
        }
    }

    fn change(kind: ChangeKind, owned: bool, destructive: bool) -> Change {
        Change {
            kind,
            owned,
            destructive,
            resource: DesiredResource {
                kind: "dns.record".to_string(),
                id: "web1".to_string(),
                attributes: serde_json::json!({}),
            },
        }
    }

    #[test]
    fn a_read_only_provider_changes_nothing() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::None),
            &[change(ChangeKind::Create, true, false)],
        );
        assert!(approved.is_empty());
        assert_eq!(refused.len(), 1);
    }

    #[test]
    fn a_destructive_change_on_an_unowned_resource_is_refused() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Delete, false, true)],
        );
        assert!(approved.is_empty());
        assert!(refused[0].contains("unowned"));
    }

    #[test]
    fn a_delete_counts_as_destructive_even_if_the_provider_says_otherwise() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Delete, false, false)],
        );
        assert!(approved.is_empty());
        assert_eq!(refused.len(), 1);
    }

    #[test]
    fn an_owned_create_is_approved() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Create, true, false)],
        );
        assert_eq!(approved.len(), 1);
        assert!(refused.is_empty());
    }

    #[test]
    fn desired_state_accepts_a_bare_array_and_an_object() {
        let array = r#"[{"kind":"dns.record","id":"a"}]"#;
        let object = r#"{"resources":[{"kind":"dns.record","id":"a"}]}"#;
        assert_eq!(parse_desired(array).unwrap().len(), 1);
        assert_eq!(parse_desired(object).unwrap().len(), 1);
        assert!(parse_desired("{}").is_err());
    }
}
