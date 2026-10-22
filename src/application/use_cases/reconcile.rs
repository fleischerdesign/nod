//! `nod reconcile`: observe, plan, approve, apply, verify (ADR-030).
//!
//! The provider describes the world; this use case decides what happens to it.
//! The policy is deliberately small and lives here, not in the provider:
//! a read-only provider may not change anything, and a destructive change on an
//! unowned resource is refused no matter who proposed it.

use std::path::Path;
use std::time::Duration;

use crate::domain::errors::NodError;
use crate::domain::policy::Policy;
use crate::domain::provider::{
    matches_expected, AppliedChange, Change, ChangeKind, DesiredResource, Handshake, InverseChange,
    ObservedResource,
};
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
    /// Provider name.
    pub provider: String,
    /// Provider implementation version.
    pub version: String,
    /// Resources the provider reported.
    pub observed: usize,
    /// Resources the declaration asks for.
    pub desired: usize,
    /// Changes the provider proposed.
    pub planned: usize,
    /// Changes the policy approved.
    pub approved: Vec<Change>,
    /// Changes the policy refused, with the reason.
    pub refused: Vec<String>,
    /// Changes the provider reported as applied.
    pub applied: usize,
    /// Changes that failed, with the provider's classification.
    pub failed: Vec<String>,
    /// Desired resources that match the declaration after the run.
    pub verified: usize,
    /// Inverse of what was applied, for `nod reconcile --rollback`.
    pub inverse: Vec<InverseChange>,
}

/// Decides which proposed changes may run.
///
/// The rules, in order:
/// 1. A read-only provider changes nothing.
/// 2. A destructive change on a resource the fleet does not own is refused.
/// 3. A delete of a protected id is refused even when owned.
/// 4. A plan that would delete more than the cap is refused **as a whole**, so
///    nothing half-applies.
pub fn approve(
    handshake: &Handshake,
    changes: &[Change],
    policy: &Policy,
) -> (Vec<Change>, Vec<String>) {
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
        if change.kind == ChangeKind::Delete && policy.is_protected(&change.resource.id) {
            refused.push(format!("{label}: protected resource"));
            continue;
        }

        approved.push(change.clone());
    }

    let deletions = approved
        .iter()
        .filter(|change| change.kind == ChangeKind::Delete)
        .count();
    if deletions > policy.max_deletions {
        let mut all_refused = refused;
        all_refused.insert(
            0,
            format!(
                "plan would delete {deletions} resources, the cap is {}; nothing was applied",
                policy.max_deletions
            ),
        );
        return (Vec::new(), all_refused);
    }

    (approved, refused)
}

/// Runs one reconcile cycle against one provider.
pub async fn execute(
    command: &Path,
    settings: &serde_json::Value,
    desired: &[DesiredResource],
    options: ReconcileOptions,
    policy: &Policy,
    env: &[String],
) -> Result<ReconcileReport, NodError> {
    let (mut provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT, env).await?;

    let observed = provider.observe(settings).await?;
    check_capabilities(&handshake, &observed.resources, &[])?;
    let plan = provider
        .plan(settings, desired, &observed.resources)
        .await?;
    check_capabilities(&handshake, &[], &plan.changes)?;
    let (approved, refused) = approve(&handshake, &plan.changes, policy);

    let mut applied = 0;
    let mut failed = Vec::new();
    let mut inverse = Vec::new();
    if options.apply && !approved.is_empty() {
        let result = provider.apply(settings, &approved).await?;

        // A change counts as applied only when the provider reports it as ok.
        // A change with no result at all is unproven, and unproven is not done.
        let mut succeeded: Vec<Change> = Vec::new();
        for change in &approved {
            let id = &change.resource.id;
            match result.applied.iter().find(|entry| &entry.id == id) {
                Some(entry) if entry.ok => succeeded.push(change.clone()),
                Some(entry) => failed.push(format!("{id}: {}", describe_failure(entry))),
                None => failed.push(format!("{id}: the provider reported no result")),
            }
        }

        applied = succeeded.len();
        if !succeeded.is_empty() {
            // Undo only what actually happened, computed from the observation
            // taken before the apply.
            inverse = crate::domain::provider::invert(&succeeded, &observed.resources);
        }
    }

    // Verification is a second observation, never "the API said 200".
    let after = provider.observe(settings).await?;
    let verified = desired
        .iter()
        .filter(|want| {
            after
                .resources
                .iter()
                .any(|resource| resource.id == want.id && matches_expected(resource, want))
        })
        .count();

    Ok(ReconcileReport {
        provider: handshake.provider.clone(),
        version: handshake.version.clone(),
        observed: observed.resources.len(),
        desired: desired.len(),
        planned: plan.changes.len(),
        approved,
        refused,
        applied,
        failed,
        verified,
        inverse,
    })
}

/// Rejects a provider that reports outside its declared capabilities.
///
/// An empty list means the provider declares nothing; enforcement is opt-in so
/// an adapter without capabilities keeps working. A non-empty list is a
/// promise, and the core is the only place that can check it.
pub fn check_capabilities(
    handshake: &Handshake,
    observed: &[ObservedResource],
    changes: &[Change],
) -> Result<(), NodError> {
    if handshake.capabilities.is_empty() {
        return Ok(());
    }
    let declared = |kind: &str| handshake.capabilities.iter().any(|entry| entry == kind);

    for resource in observed {
        if !declared(&resource.kind) {
            return Err(NodError::provider(format!(
                "provider {} declared capabilities [{}] but observed kind `{}` (resource `{}`)",
                handshake.provider,
                handshake.capabilities.join(", "),
                resource.kind,
                resource.id
            )));
        }
    }
    for change in changes {
        if !declared(&change.resource.kind) {
            return Err(NodError::provider(format!(
                "provider {} declared capabilities [{}] but proposed kind `{}` (resource `{}`)",
                handshake.provider,
                handshake.capabilities.join(", "),
                change.resource.kind,
                change.resource.id
            )));
        }
    }
    Ok(())
}

/// Renders one provider-reported failure with its classification when given.
fn describe_failure(entry: &AppliedChange) -> String {
    match &entry.error {
        Some(error) if !error.message.is_empty() => format!("{}: {}", error.kind, error.message),
        Some(error) => error.kind.clone(),
        None if !entry.message.is_empty() => entry.message.clone(),
        None => "apply failed".to_string(),
    }
}

/// What a rollback run produced.
#[derive(Debug, Clone)]
pub struct RollbackReport {
    /// Provider name.
    pub provider: String,
    /// Provider implementation version.
    pub version: String,
    /// Changes restored.
    pub restored: usize,
    /// Refusals, when the world moved since the apply.
    pub refused: Vec<String>,
}

/// Applies the inverse of a recorded plan.
///
/// A rollback refuses rather than overwrites: if a resource changed, vanished
/// or reappeared since the apply, undoing blindly would destroy someone else's
/// work. The refusal names the resource, so an operator can decide.
pub async fn rollback(
    command: &Path,
    settings: &serde_json::Value,
    plan: &crate::domain::provider::RollbackPlan,
    policy: &Policy,
    env: &[String],
) -> Result<RollbackReport, NodError> {
    use crate::domain::provider::matches_expected;

    let (mut provider, handshake) = ProcessProvider::open(command, OP_TIMEOUT, env).await?;
    let observed = provider.observe(settings).await?;

    let mut refused = Vec::new();
    for entry in &plan.changes {
        let id = &entry.change.resource.id;
        let current = observed
            .resources
            .iter()
            .find(|resource| &resource.id == id);
        match (&entry.expected, current) {
            (Some(expected), Some(current)) if matches_expected(current, expected) => {}
            (Some(_), Some(_)) => refused.push(format!("{id}: changed since the apply")),
            (Some(_), None) => refused.push(format!("{id}: gone since the apply")),
            (None, Some(_)) => refused.push(format!("{id}: exists again since the apply")),
            (None, None) => {}
        }
    }

    if !refused.is_empty() {
        return Ok(RollbackReport {
            provider: handshake.provider.clone(),
            version: handshake.version.clone(),
            restored: 0,
            refused,
        });
    }

    let changes: Vec<Change> = plan
        .changes
        .iter()
        .map(|entry| entry.change.clone())
        .collect();
    // A rollback is operator-initiated and undoes a recorded plan, so the
    // per-run deletion cap does not apply. The protect list still does: a name
    // we never delete is not deleted by a rollback either.
    let relaxed = Policy {
        protect: policy.protect.clone(),
        max_deletions: usize::MAX,
    };
    let (approved, refused) = approve(&handshake, &changes, &relaxed);
    if !refused.is_empty() {
        return Ok(RollbackReport {
            provider: handshake.provider.clone(),
            version: handshake.version.clone(),
            restored: 0,
            refused,
        });
    }

    let applied = provider.apply(settings, &approved).await?;
    Ok(RollbackReport {
        provider: handshake.provider.clone(),
        version: handshake.version.clone(),
        restored: applied.applied.iter().filter(|entry| entry.ok).count(),
        refused: Vec::new(),
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
    use crate::domain::provider::{
        AppliedChange, DesiredResource, ObservedResource, OwnershipMode, ProviderError,
    };

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
        change_named(kind, "web1", owned, destructive)
    }

    fn change_named(kind: ChangeKind, id: &str, owned: bool, destructive: bool) -> Change {
        Change {
            kind,
            owned,
            destructive,
            resource: DesiredResource {
                kind: "dns.record".to_string(),
                id: id.to_string(),
                attributes: serde_json::json!({}),
            },
        }
    }

    /// Strict default: no pattern, no deletion allowed.
    fn strict() -> Policy {
        Policy::default()
    }

    fn allows(max_deletions: usize, protect: &[&str]) -> Policy {
        Policy {
            protect: protect.iter().map(|entry| entry.to_string()).collect(),
            max_deletions,
        }
    }

    #[test]
    fn a_read_only_provider_changes_nothing() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::None),
            &[change(ChangeKind::Create, true, false)],
            &allows(1, &[]),
        );
        assert!(approved.is_empty());
        assert_eq!(refused.len(), 1);
    }

    #[test]
    fn a_destructive_change_on_an_unowned_resource_is_refused() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Delete, false, true)],
            &allows(1, &[]),
        );
        assert!(approved.is_empty());
        assert!(refused[0].contains("unowned"));
    }

    #[test]
    fn a_delete_counts_as_destructive_even_if_the_provider_says_otherwise() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Delete, false, false)],
            &allows(1, &[]),
        );
        assert!(approved.is_empty());
        assert_eq!(refused.len(), 1);
    }

    #[test]
    fn an_owned_create_is_approved() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Create, true, false)],
            &strict(),
        );
        assert_eq!(approved.len(), 1);
        assert!(refused.is_empty());
    }

    #[test]
    fn an_owned_delete_is_refused_by_default() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Delete, true, true)],
            &strict(),
        );
        assert!(approved.is_empty());
        assert!(refused[0].contains("cap is 0"), "{}", refused[0]);
    }

    #[test]
    fn an_owned_delete_within_the_cap_is_approved() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Delete, true, true)],
            &allows(1, &[]),
        );
        assert_eq!(approved.len(), 1);
        assert!(refused.is_empty());
    }

    #[test]
    fn a_protected_owned_resource_is_never_deleted() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change_named(
                ChangeKind::Delete,
                "A:example.com",
                true,
                true,
            )],
            &allows(10, &["*:example.com"]),
        );
        assert!(approved.is_empty());
        assert!(refused[0].contains("protected"), "{}", refused[0]);
    }

    #[test]
    fn a_protected_resource_may_still_be_updated() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change_named(
                ChangeKind::Update,
                "A:example.com",
                true,
                false,
            )],
            &allows(0, &["*:example.com"]),
        );
        assert_eq!(approved.len(), 1);
        assert!(refused.is_empty());
    }

    #[test]
    fn a_plan_over_the_cap_is_refused_as_a_whole() {
        let changes: Vec<Change> = (0..3)
            .map(|index| {
                change_named(
                    ChangeKind::Delete,
                    &format!("TXT:old{index}.example.com"),
                    true,
                    true,
                )
            })
            .chain(std::iter::once(change(ChangeKind::Create, true, false)))
            .collect();

        let (approved, refused) =
            approve(&handshake(OwnershipMode::Marker), &changes, &allows(1, &[]));
        assert!(approved.is_empty(), "a refused plan must apply nothing");
        assert!(refused[0].contains("would delete 3"), "{}", refused[0]);
        assert!(refused
            .iter()
            .any(|reason| reason.contains("nothing was applied")));
    }

    #[test]
    fn a_plan_exactly_at_the_cap_passes() {
        let changes = vec![
            change_named(ChangeKind::Delete, "TXT:a.example.com", true, true),
            change_named(ChangeKind::Delete, "TXT:b.example.com", true, true),
        ];
        let (approved, refused) =
            approve(&handshake(OwnershipMode::Marker), &changes, &allows(2, &[]));
        assert_eq!(approved.len(), 2);
        assert!(refused.is_empty());
    }

    #[test]
    fn a_provider_that_reports_outside_its_capabilities_is_rejected() {
        let mut handshake = handshake(OwnershipMode::Marker);
        handshake.capabilities = vec!["dns.record".to_string()];
        let observed = vec![ObservedResource {
            kind: "mail.mailbox".to_string(),
            id: "inbox".to_string(),
            attributes: serde_json::json!({}),
            owned: true,
        }];

        let error = check_capabilities(&handshake, &observed, &[])
            .expect_err("an undeclared kind must be a protocol violation");
        assert!(error.to_string().contains("mail.mailbox"), "{error}");
    }

    #[test]
    fn an_empty_capability_list_is_not_enforced() {
        let handshake = handshake(OwnershipMode::Marker);
        assert!(handshake.capabilities.is_empty());
        let observed = vec![ObservedResource {
            kind: "anything".to_string(),
            id: "x".to_string(),
            attributes: serde_json::json!({}),
            owned: true,
        }];
        check_capabilities(&handshake, &observed, &[]).unwrap();
    }

    #[test]
    fn a_change_outside_the_declared_capabilities_is_rejected() {
        let mut handshake = handshake(OwnershipMode::Marker);
        handshake.capabilities = vec!["dns.record".to_string()];
        let mut proposal = change(ChangeKind::Create, true, false);
        proposal.resource.kind = "mail.mailbox".to_string();

        let error = check_capabilities(&handshake, &[], &[proposal])
            .expect_err("a proposed undeclared kind must be a protocol violation");
        assert!(error.to_string().contains("proposed kind"), "{error}");
    }

    #[test]
    fn a_failure_is_described_by_its_class_and_message() {
        let entry = AppliedChange {
            id: "web1".to_string(),
            ok: false,
            message: "legacy text".to_string(),
            error: Some(ProviderError {
                kind: "auth".to_string(),
                message: "token expired".to_string(),
                retry_after_secs: None,
            }),
        };
        assert_eq!(describe_failure(&entry), "auth: token expired");
    }

    #[test]
    fn an_unclassified_failure_falls_back_to_its_message() {
        let legacy = AppliedChange {
            id: "web1".to_string(),
            ok: false,
            message: "quota exceeded".to_string(),
            error: None,
        };
        assert_eq!(describe_failure(&legacy), "quota exceeded");

        let bare = AppliedChange {
            id: "web1".to_string(),
            ok: false,
            message: String::new(),
            error: None,
        };
        assert_eq!(describe_failure(&bare), "apply failed");
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
