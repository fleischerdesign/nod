//! `nod reconcile`: observe, plan, approve, apply, verify (ADR-030).
//!
//! The provider describes the world; this use case decides what happens to it.
//! The policy is deliberately small and lives here, not in the provider:
//! a read-only provider may not change anything, and a destructive change on an
//! unowned resource is refused no matter who proposed it.

use std::path::Path;
use std::time::Duration;

use super::derive;
use crate::domain::errors::NodError;
use crate::domain::policy::Policy;
use crate::domain::provider::{
    AppliedChange, Change, ChangeKind, Condition, DesiredResource, Handshake, InverseChange,
    ObservedResource,
};
use crate::infrastructure::provider::process::ProcessProvider;

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
    /// Declared resources that exist and match but do not carry the marker.
    pub unowned: usize,
    /// Resources the declaration asks for.
    pub desired: usize,
    /// Resources materialised by a declared derivation.
    pub derived: usize,
    /// The derived set this run declared, for the next run's budget.
    pub derivations: crate::domain::derivation::Derivations,
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
    /// Deletions the declaration authorized.
    pub declared_deletions: usize,
    /// Deletions the plan proposed without a declaration.
    pub implicit_deletions: usize,
    /// Conditions the provider reported, deduplicated across observe and apply.
    ///
    /// A condition is not an error: the provider can still observe and plan,
    /// but something about its credential or upstream limits needs attention.
    pub conditions: Vec<Condition>,
    /// Inverse of what was applied, for `nod reconcile --rollback`.
    pub inverse: Vec<InverseChange>,
}

/// Appends the conditions the provider has not reported yet.
fn merge_conditions(into: &mut Vec<Condition>, from: &[Condition]) {
    for condition in from {
        if !into.iter().any(|existing| existing == condition) {
            into.push(condition.clone());
        }
    }
}

/// Decides which proposed changes may run.
///
/// The rules, in order:
/// 1. A read-only provider changes nothing.
/// 2. A change to a resource the fleet does not own is refused — create,
///    update and delete alike. A foreign record is reported, never modified.
/// 3. A delete of a protected id is refused even when owned.
/// 4. A plan that would delete more **undeclared** resources than the cap is
///    refused as a whole, so nothing half-applies. A deletion the declaration
///    names in `absent`, or one a derivation rule produced and its own budget
///    allows, is authorized and does not consume the global cap; the cap exists
///    to bound a declaration bug, not an intention.
pub fn approve(
    handshake: &Handshake,
    changes: &[Change],
    policy: &Policy,
) -> (Vec<Change>, Vec<String>) {
    approve_declared(handshake, changes, policy, &Declared::default())
}

/// Decides which proposed changes may run, knowing which deletions the
/// declaration authorizes.
///
/// The forward path calls this; `approve` is the same decision without a
/// declaration context, which is what rollback and the policy tests need.
pub fn approve_declared(
    handshake: &Handshake,
    changes: &[Change],
    policy: &Policy,
    declared: &Declared,
) -> (Vec<Change>, Vec<String>) {
    let mut approved = Vec::new();
    let mut refused = Vec::new();

    for change in changes {
        let label = format!("{:?} {}", change.kind, change.resource.id);

        if !handshake.ownership.allows_writes() {
            refused.push(format!("{label}: provider is read-only"));
            continue;
        }
        if !change.owned {
            refused.push(format!("{label}: unowned resource"));
            continue;
        }
        if change.kind == ChangeKind::Delete && policy.is_protected(&change.resource.id) {
            refused.push(format!("{label}: protected resource"));
            continue;
        }

        approved.push(change.clone());
    }

    let mut implicit = 0usize;
    let mut explicit = 0usize;
    let mut derived: std::collections::BTreeMap<&str, (usize, usize)> =
        std::collections::BTreeMap::new();
    for change in &approved {
        if change.kind != ChangeKind::Delete {
            continue;
        }
        if declared.is_explicit(&change.resource.kind, &change.resource.id) {
            explicit += 1;
        } else if let Some((rule, budget)) =
            declared.derived_budget(&change.resource.kind, &change.resource.id)
        {
            let entry = derived.entry(rule).or_insert((0, budget));
            entry.0 += 1;
        } else {
            implicit += 1;
        }
    }

    let over: Vec<String> = derived
        .iter()
        .filter(|(_, (count, budget))| count > budget)
        .map(|(rule, (count, budget))| {
            format!("derivation `{rule}` would remove {count} resources, the cap is {budget}")
        })
        .collect();
    if !over.is_empty() {
        let mut all_refused = refused;
        for message in over {
            all_refused.insert(0, message);
        }
        all_refused.push("nothing was applied".to_string());
        return (Vec::new(), all_refused);
    }

    let declared_total = explicit + derived.values().map(|(count, _)| count).sum::<usize>();
    if implicit > policy.max_deletions {
        let mut all_refused = refused;
        all_refused.insert(
            0,
            format!(
                "plan would delete {implicit} undeclared resources ({declared_total} declared), the cap is {}; nothing was applied",
                policy.max_deletions
            ),
        );
        return (Vec::new(), all_refused);
    }

    (approved, refused)
}

/// Verifies that every declared absence is satisfiable.
///
/// A foreign resource cannot be deleted by this fleet, and a provider that
/// plans no deletion cannot carry the declaration out; both are configuration
/// errors that must fail before anything is applied. An entry whose resource is
/// already gone is satisfied and needs no change.
fn check_absent(
    absent: &[DesiredResource],
    observed: &[ObservedResource],
    changes: &[Change],
) -> Result<(), NodError> {
    for entry in absent {
        let Some(resource) = observed
            .iter()
            .find(|resource| resource.kind == entry.kind && resource.id == entry.id)
        else {
            continue;
        };
        if !resource.owned {
            return Err(NodError::config(format!(
                "declaration marks {} {} absent, but the fleet does not own it; refusing to delete a foreign resource",
                entry.kind, entry.id
            )));
        }
        let planned = changes.iter().any(|change| {
            change.kind == ChangeKind::Delete
                && change.resource.kind == entry.kind
                && change.resource.id == entry.id
        });
        if !planned {
            return Err(NodError::config(format!(
                "declaration marks {} {} absent, but the provider planned no deletion for it",
                entry.kind, entry.id
            )));
        }
    }
    Ok(())
}

/// Runs one reconcile cycle against one provider.
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    command: &Path,
    settings: &serde_json::Value,
    desired: &Desired,
    record: &crate::domain::derivation::Derivations,
    options: ReconcileOptions,
    policy: &Policy,
    env: &[String],
    timeout: Duration,
) -> Result<ReconcileReport, NodError> {
    let (mut provider, handshake) = ProcessProvider::open(command, timeout, env).await?;

    let observed = provider.observe(settings).await?;
    check_capabilities(&handshake, &observed.resources, &[])?;
    let mut conditions = Vec::new();
    merge_conditions(&mut conditions, &observed.conditions);
    // A derivation reads a successful observation. A source or attribute that
    // is missing is a configuration error, never an empty set (ADR-036).
    let derived = derive::expand(&desired.derive, &observed.resources, &desired.resources)?;
    let mut effective = desired.resources.clone();
    effective.extend(derived.iter().map(|entry| entry.resource.clone()));

    // What this declaration derives, for the next run's deletion budget. It is
    // the declared set, not the applied one: a failed create has no delete to
    // authorize, and the record must follow the rule, not the API's answer.
    let mut derivations = crate::domain::derivation::Derivations {
        provider: handshake.provider.clone(),
        ..Default::default()
    };
    for entry in &derived {
        derivations
            .rules
            .entry(entry.rule.clone())
            .or_default()
            .push(entry.resource.id.clone());
    }

    let plan = provider
        .plan(settings, &effective, &observed.resources)
        .await?;
    check_capabilities(&handshake, &[], &plan.changes)?;
    merge_conditions(&mut conditions, &plan.conditions);
    let changed_before: Vec<(String, String)> = plan
        .changes
        .iter()
        .map(|change| (change.resource.kind.clone(), change.resource.id.clone()))
        .collect();

    let declared = desired.declared(record);
    check_absent(&desired.absent, &observed.resources, &plan.changes)?;
    let (approved, refused) = approve_declared(&handshake, &plan.changes, policy, &declared);

    let mut applied = 0;
    let mut failed = Vec::new();
    let mut succeeded: Vec<Change> = Vec::new();
    if options.apply && !approved.is_empty() {
        let result = provider.apply(settings, &approved).await?;
        merge_conditions(&mut conditions, &result.conditions);

        // A change counts as applied only when the provider reports it as ok.
        // A change with no result at all is unproven, and unproven is not done.
        for change in &approved {
            let id = &change.resource.id;
            match result.applied.iter().find(|entry| &entry.id == id) {
                Some(entry) if entry.ok => succeeded.push(change.clone()),
                Some(entry) => failed.push(format!("{id}: {}", describe_failure(entry))),
                None => failed.push(format!("{id}: the provider reported no result")),
            }
        }
        applied = succeeded.len();
    }

    // Verification is a second observation, never "the API said 200". It is
    // also the only place that sees the provider's own id for a created
    // resource, which the recorded undo needs.
    let after = provider.observe(settings).await?;
    merge_conditions(&mut conditions, &after.conditions);
    let inverse = if succeeded.is_empty() {
        Vec::new()
    } else {
        crate::domain::provider::invert(&succeeded, &observed.resources, &after.resources)
    };

    // Verification is the provider's own plan against the post-activation
    // observation, not an attribute comparison in the core: a seed's profile is
    // history, and only the provider knows what its declaration enforces.
    let after_plan = provider
        .plan(settings, &effective, &after.resources)
        .await?;
    let verified = effective
        .iter()
        .filter(|want| {
            !after_plan
                .changes
                .iter()
                .any(|change| change.resource.kind == want.kind && change.resource.id == want.id)
        })
        .count();

    let declared_deletions = plan
        .changes
        .iter()
        .filter(|change| {
            change.kind == ChangeKind::Delete
                && declared.authorizes(&change.resource.kind, &change.resource.id)
        })
        .count();
    let implicit_deletions = plan
        .changes
        .iter()
        .filter(|change| {
            change.kind == ChangeKind::Delete
                && !declared.authorizes(&change.resource.kind, &change.resource.id)
        })
        .count();

    Ok(ReconcileReport {
        provider: handshake.provider.clone(),
        version: handshake.version.clone(),
        observed: observed.resources.len(),
        unowned: crate::domain::adoption::unowned(&observed.resources, &effective, &changed_before),
        desired: effective.len(),
        derived: derived.len(),
        derivations,
        planned: plan.changes.len(),
        approved,
        refused,
        applied,
        failed,
        verified,
        declared_deletions,
        implicit_deletions,
        conditions,
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
pub(crate) fn describe_failure(entry: &AppliedChange) -> String {
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
    /// Changes the provider could not restore, classified when it said why.
    pub failed: Vec<String>,
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
    timeout: Duration,
) -> Result<RollbackReport, NodError> {
    use crate::domain::provider::matches_expected;

    let (mut provider, handshake) = ProcessProvider::open(command, timeout, env).await?;
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
            failed: Vec::new(),
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
            failed: Vec::new(),
        });
    }

    let applied = provider.apply(settings, &approved).await?;

    // An undo that did not happen must never look like one that did: the plan
    // stays on disk until every recorded change is restored.
    let mut restored = 0;
    let mut failed = Vec::new();
    for change in &approved {
        let id = &change.resource.id;
        match applied.applied.iter().find(|entry| &entry.id == id) {
            Some(entry) if entry.ok => restored += 1,
            Some(entry) => failed.push(format!("{id}: {}", describe_failure(entry))),
            None => failed.push(format!("{id}: the provider reported no result")),
        }
    }

    Ok(RollbackReport {
        provider: handshake.provider.clone(),
        version: handshake.version.clone(),
        restored,
        refused: Vec::new(),
        failed,
    })
}

/// Parses a desired-state document: either a bare array or `{ "resources": [...] }`.
/// A parsed declaration: what must exist and what must not.
#[derive(Debug, Clone, Default)]
pub struct Desired {
    /// Resources the provider must create or keep in shape.
    pub resources: Vec<DesiredResource>,
    /// Resources the declaration says must not exist.
    ///
    /// The provider never sees this list: it plans from `resources` alone, and
    /// the delete it proposes for an owned resource outside that list is
    /// reclassified by the core. A separate list keeps an old provider from
    /// mistaking a tombstone for something to create.
    pub absent: Vec<DesiredResource>,
    /// Declared derivations: rules that materialise a set from an observed fact
    /// (ADR-036). The provider never sees them; the core expands them into
    /// `resources` before planning.
    pub derive: Vec<derive::DeriveRule>,
}

impl Desired {
    /// The deletions this declaration authorizes, explicit and derived.
    fn declared(&self, record: &crate::domain::derivation::Derivations) -> Declared {
        Declared {
            entries: self.absent.clone(),
            derived: self
                .derive
                .iter()
                .map(|rule| DerivedBudget {
                    rule: rule.id.clone(),
                    kind: rule.template.kind.clone(),
                    ids: record.rule(&rule.id).to_vec(),
                    max_deletions: rule.max_deletions,
                })
                .collect(),
        }
    }
}

/// The deletions a declaration authorizes.
#[derive(Debug, Clone, Default)]
pub struct Declared {
    /// Explicit tombstones.
    entries: Vec<DesiredResource>,
    /// What each rule produced last time, and how many of those it may remove.
    derived: Vec<DerivedBudget>,
}

/// One rule's derived set and its removal budget.
#[derive(Debug, Clone)]
struct DerivedBudget {
    rule: String,
    kind: String,
    ids: Vec<String>,
    max_deletions: usize,
}

impl Declared {
    /// Whether `kind`/`id` is an explicit tombstone.
    fn is_explicit(&self, kind: &str, id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.kind == kind && entry.id == id)
    }

    /// The rule that produced `kind`/`id` last time, with its budget.
    fn derived_budget(&self, kind: &str, id: &str) -> Option<(&str, usize)> {
        self.derived
            .iter()
            .find(|budget| budget.kind == kind && budget.ids.iter().any(|known| known == id))
            .map(|budget| (budget.rule.as_str(), budget.max_deletions))
    }

    /// Whether the declaration authorizes this deletion, explicitly or by rule.
    fn authorizes(&self, kind: &str, id: &str) -> bool {
        self.is_explicit(kind, id) || self.derived_budget(kind, id).is_some()
    }
}

/// Parses a declaration from the desired-state file.
pub fn parse_desired(raw: &str) -> Result<Desired, NodError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| NodError::config(format!("cannot parse desired state: {error}")))?;

    match value {
        serde_json::Value::Array(items) => Ok(Desired {
            resources: parse_resources(items)?,
            absent: Vec::new(),
            derive: Vec::new(),
        }),
        serde_json::Value::Object(mut map) => {
            let schema = match map.remove("schema") {
                None | Some(serde_json::Value::Null) => 1,
                Some(serde_json::Value::Number(number)) => number.as_u64().ok_or_else(|| {
                    NodError::config("desired state `schema` must be a positive integer")
                })?,
                Some(_) => {
                    return Err(NodError::config(
                        "desired state `schema` must be a positive integer",
                    ))
                }
            };
            if schema > derive::SCHEMA {
                return Err(NodError::config(format!(
                    "desired state declares schema {schema}, this core understands {}",
                    derive::SCHEMA
                )));
            }
            let resources = match map.remove("resources") {
                Some(serde_json::Value::Array(items)) => items,
                _ => {
                    return Err(NodError::config(
                        "desired state object must contain a `resources` array",
                    ))
                }
            };
            let absent = match map.remove("absent") {
                Some(serde_json::Value::Array(items)) => parse_resources(items)?,
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(_) => return Err(NodError::config("desired state `absent` must be an array")),
            };
            let derive = match map.remove("derive") {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(value) => {
                    if schema < 2 {
                        return Err(NodError::config(
                            "desired state uses `derive`, which requires schema 2",
                        ));
                    }
                    derive::parse(&value)?
                }
            };
            Ok(Desired {
                resources: parse_resources(resources)?,
                absent,
                derive,
            })
        }
        _ => Err(NodError::config(
            "desired state must be an array or an object with `resources`",
        )),
    }
}

/// Parses a list of desired resources.
fn parse_resources(items: Vec<serde_json::Value>) -> Result<Vec<DesiredResource>, NodError> {
    items
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
            operations: Vec::new(),
            required_permissions: Vec::new(),
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
    fn an_update_of_an_unowned_resource_is_refused() {
        let (approved, refused) = approve(
            &handshake(OwnershipMode::Marker),
            &[change(ChangeKind::Update, false, false)],
            &allows(0, &[]),
        );
        assert!(approved.is_empty());
        assert!(refused[0].contains("unowned"), "{}", refused[0]);
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

    fn derived_declared(rule: &str, kind: &str, id: &str, max_deletions: usize) -> Declared {
        Declared {
            entries: Vec::new(),
            derived: vec![DerivedBudget {
                rule: rule.to_string(),
                kind: kind.to_string(),
                ids: vec![id.to_string()],
                max_deletions,
            }],
        }
    }

    #[test]
    fn a_derived_deletion_within_its_budget_is_approved() {
        let changes = vec![change_named(ChangeKind::Delete, "old", true, true)];
        let (approved, refused) = approve_declared(
            &handshake(OwnershipMode::Marker),
            &changes,
            &allows(0, &[]),
            &derived_declared("per-member", "dns.record", "old", 1),
        );
        assert_eq!(approved.len(), 1);
        assert!(refused.is_empty());
    }

    #[test]
    fn a_derived_deletion_over_its_budget_refuses_the_plan() {
        let changes = vec![change_named(ChangeKind::Delete, "old", true, true)];
        let (approved, refused) = approve_declared(
            &handshake(OwnershipMode::Marker),
            &changes,
            &allows(0, &[]),
            &derived_declared("per-member", "dns.record", "old", 0),
        );
        assert!(approved.is_empty(), "a refused plan must apply nothing");
        assert!(
            refused[0].contains("derivation `per-member`"),
            "{}",
            refused[0]
        );
    }

    #[test]
    fn a_delete_outside_the_derived_set_still_uses_the_global_cap() {
        let changes = vec![change_named(ChangeKind::Delete, "other", true, true)];
        let (approved, refused) = approve_declared(
            &handshake(OwnershipMode::Marker),
            &changes,
            &allows(0, &[]),
            &derived_declared("per-member", "dns.record", "old", 1),
        );
        assert!(approved.is_empty());
        assert!(
            refused[0].contains("undeclared resources"),
            "{}",
            refused[0]
        );
    }

    #[test]
    fn the_declaration_builds_its_derived_budget_from_the_record() {
        let desired = Desired {
            resources: Vec::new(),
            absent: Vec::new(),
            derive: vec![derive::DeriveRule {
                id: "per-member".to_string(),
                from: derive::Source {
                    kind: "authentik.group".to_string(),
                    id: "ai".to_string(),
                    attribute: "users".to_string(),
                },
                template: derive::Template {
                    kind: "dns.record".to_string(),
                    id: "{member}".to_string(),
                    attributes: serde_json::json!({}),
                },
                max_deletions: 1,
            }],
        };
        let mut record = crate::domain::derivation::Derivations::default();
        record
            .rules
            .insert("per-member".to_string(), vec!["old".to_string()]);

        let declared = desired.declared(&record);
        assert!(!declared.is_explicit("dns.record", "old"));
        assert_eq!(
            declared.derived_budget("dns.record", "old"),
            Some(("per-member", 1))
        );
    }

    fn declared(entries: &[(&str, &str)]) -> Declared {
        Declared {
            entries: entries
                .iter()
                .map(|(kind, id)| DesiredResource {
                    kind: (*kind).to_string(),
                    id: (*id).to_string(),
                    attributes: serde_json::json!({}),
                })
                .collect(),
            derived: Vec::new(),
        }
    }

    fn observed_owned(id: &str) -> ObservedResource {
        ObservedResource {
            kind: "dns.record".to_string(),
            id: id.to_string(),
            attributes: serde_json::json!({}),
            owned: true,
        }
    }

    #[test]
    fn a_declared_deletion_does_not_consume_the_cap() {
        let changes = vec![change_named(ChangeKind::Delete, "old", true, true)];
        let (approved, refused) = approve_declared(
            &handshake(OwnershipMode::Marker),
            &changes,
            &allows(0, &[]),
            &declared(&[("dns.record", "old")]),
        );
        assert_eq!(approved.len(), 1);
        assert!(refused.is_empty());
    }

    #[test]
    fn an_implicit_deletion_over_the_cap_refuses_the_whole_plan() {
        let changes = vec![
            change_named(ChangeKind::Delete, "old", true, true),
            change_named(ChangeKind::Delete, "retired", true, true),
            change(ChangeKind::Create, true, false),
        ];
        let (approved, refused) = approve_declared(
            &handshake(OwnershipMode::Marker),
            &changes,
            &allows(0, &[]),
            &declared(&[("dns.record", "retired")]),
        );
        assert!(approved.is_empty(), "a refused plan must apply nothing");
        assert!(refused[0].contains("1 undeclared"), "{}", refused[0]);
        assert!(refused[0].contains("1 declared"), "{}", refused[0]);
    }

    #[test]
    fn a_declared_deletion_of_a_protected_resource_is_still_refused() {
        let changes = vec![change_named(
            ChangeKind::Delete,
            "TXT:a.example.com",
            true,
            true,
        )];
        let (approved, refused) = approve_declared(
            &handshake(OwnershipMode::Marker),
            &changes,
            &allows(0, &["TXT:a.example.com"]),
            &declared(&[("dns.record", "TXT:a.example.com")]),
        );
        assert!(approved.is_empty());
        assert!(refused[0].contains("protected"), "{}", refused[0]);
    }

    #[test]
    fn an_absent_entry_for_a_foreign_resource_is_an_error() {
        let mut foreign = observed_owned("old");
        foreign.owned = false;
        let error = check_absent(&declared(&[("dns.record", "old")]).entries, &[foreign], &[])
            .expect_err("a foreign resource cannot be declared absent");
        assert!(error.to_string().contains("does not own"), "{error}");
    }

    #[test]
    fn an_absent_entry_without_a_planned_deletion_is_an_error() {
        let error = check_absent(
            &declared(&[("dns.record", "old")]).entries,
            &[observed_owned("old")],
            &[],
        )
        .expect_err("a declaration the provider cannot carry out must fail");
        assert!(error.to_string().contains("planned no deletion"), "{error}");
    }

    #[test]
    fn an_absent_entry_that_is_already_gone_is_satisfied() {
        assert!(check_absent(&declared(&[("dns.record", "old")]).entries, &[], &[]).is_ok());
    }

    #[test]
    fn a_declared_deletion_the_provider_planned_is_accepted() {
        let changes = vec![change_named(ChangeKind::Delete, "old", true, true)];
        assert!(check_absent(
            &declared(&[("dns.record", "old")]).entries,
            &[observed_owned("old")],
            &changes,
        )
        .is_ok());
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
        assert_eq!(parse_desired(array).unwrap().resources.len(), 1);
        assert_eq!(parse_desired(object).unwrap().resources.len(), 1);
        assert!(parse_desired("{}").is_err());
    }

    #[test]
    fn desired_state_carries_the_absent_list() {
        let raw = r#"{"resources":[{"kind":"dns.record","id":"a"}],"absent":[{"kind":"dns.record","id":"old"}]}"#;
        let desired = parse_desired(raw).unwrap();
        assert_eq!(desired.resources.len(), 1);
        assert_eq!(desired.absent.len(), 1);
        assert_eq!(desired.absent[0].id, "old");
        assert!(parse_desired(r#"{"resources":[],"absent":{}}"#).is_err());
    }

    #[test]
    fn derive_requires_schema_two_and_a_known_schema() {
        let valid = r#"{"schema":2,"resources":[],"derive":[{"id":"r","from":{"kind":"k","id":"i","attribute":"a"},"template":{"kind":"t","id":"{member}"}}]}"#;
        assert_eq!(parse_desired(valid).unwrap().derive.len(), 1);

        let without_schema = r#"{"resources":[],"derive":[{"id":"r","from":{"kind":"k","id":"i","attribute":"a"},"template":{"kind":"t","id":"{member}"}}]}"#;
        assert!(
            parse_desired(without_schema).is_err(),
            "derive requires schema 2"
        );

        assert!(
            parse_desired(r#"{"schema":3,"resources":[]}"#).is_err(),
            "a schema this core does not know must be refused"
        );
    }
}
