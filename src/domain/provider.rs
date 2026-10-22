//! Provider protocol types (ADR-030).
//!
//! A provider is a process the core talks to in JSON Lines. These types are the
//! wire contract: the core owns them, providers implement them. Nothing here
//! may depend on a concrete provider.

use serde::{Deserialize, Serialize};

/// Wire protocol major version. A provider with another major is rejected before
/// it is started.
pub const PROTOCOL_VERSION: u32 = 1;

/// First message: the core announces the protocol it speaks.
#[derive(Debug, Clone, Serialize)]
pub struct HandshakeRequest {
    /// Protocol major version.
    pub protocol: u32,
}

/// Answer to [`HandshakeRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handshake {
    /// Provider name, e.g. `cloudflare`.
    pub provider: String,
    /// Provider implementation version, for support and `nod provider list`.
    pub version: String,
    /// Protocol major version the provider speaks.
    pub protocol: u32,
    /// What the provider can do; the core uses it to scope credentials.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Optional operations beyond observe/plan/apply, e.g. `adopt`.
    ///
    /// Additive by design: an old core ignores the field, an old provider omits
    /// it, and no protocol major changes. A core that needs `adopt` refuses a
    /// provider that does not declare it.
    #[serde(default)]
    pub operations: Vec<String>,
    /// How the provider marks ownership.
    pub ownership: OwnershipMode,
    /// JSON Schema of the provider's `settings`.
    #[serde(default)]
    pub settings_schema: serde_json::Value,
}

/// How resources are attributed to the fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OwnershipMode {
    /// Every resource carries a marker.
    Marker,
    /// The whole namespace belongs to the fleet; an explicit opt-in.
    Namespace,
    /// Read-only: the provider may never propose a change.
    None,
}

impl OwnershipMode {
    /// Whether this mode permits any write at all.
    pub fn allows_writes(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Where a provider runs relative to the host sweep (ADR-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderOrder {
    /// Runs whenever its own schedule is due.
    #[default]
    Independent,
    /// Runs immediately before a host sweep, on the sweep's cadence.
    BeforeHosts,
    /// Runs immediately after a host sweep, on the sweep's cadence.
    AfterHosts,
}

/// Static metadata shipped next to a provider binary.
///
/// `nod provider list` reads this instead of executing the provider: listing
/// what is installed must not require running it. The handshake remains the
/// runtime truth, and `nod provider test` compares the two.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Provider name; must match the handshake.
    pub name: String,
    /// Implementation version.
    pub version: String,
    /// Protocol major version.
    pub protocol: u32,
    /// Declared capabilities.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Declared optional operations (`adopt`).
    #[serde(default)]
    pub operations: Vec<String>,
    /// Declared ownership mode.
    pub ownership: OwnershipMode,
    /// What the provider manages, in one sentence.
    #[serde(default)]
    pub description: Option<String>,
    /// Write-conformance fixture.
    ///
    /// Without it, `nod provider test --write` refuses: a write test needs a
    /// namespace that is allowed to be destroyed, and only the provider knows
    /// which resources express that.
    #[serde(default)]
    pub sandbox: Option<Sandbox>,
}

/// What the write conformance creates and removes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sandbox {
    /// Resources the test creates, verifies and deletes again.
    pub desired: Vec<DesiredResource>,
}

impl Manifest {
    /// Reads a manifest from `path`.
    pub fn read(path: &std::path::Path) -> Result<Self, crate::domain::errors::NodError> {
        let raw = std::fs::read_to_string(path).map_err(|error| {
            crate::domain::errors::NodError::config(format!(
                "cannot read manifest {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_str(&raw).map_err(|error| {
            crate::domain::errors::NodError::config(format!(
                "invalid manifest {}: {error}",
                path.display()
            ))
        })
    }
}

/// A resource as the provider sees it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservedResource {
    /// Resource kind, e.g. `dns.record`.
    pub kind: String,
    /// Provider-scoped identifier.
    pub id: String,
    /// Provider-specific attributes.
    #[serde(default)]
    pub attributes: serde_json::Value,
    /// Whether the fleet owns it. Absent means `false`: unowned is the safe
    /// default, and the core never deletes what it does not own.
    #[serde(default)]
    pub owned: bool,
}

/// A resource the declaration asks for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesiredResource {
    /// Resource kind.
    pub kind: String,
    /// Provider-scoped identifier.
    pub id: String,
    /// Provider-specific attributes.
    #[serde(default)]
    pub attributes: serde_json::Value,
}

/// What a change does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    /// Create a resource that does not exist.
    Create,
    /// Update an existing resource.
    Update,
    /// Delete a resource.
    Delete,
}

/// One proposed change. `owned` and `destructive` are mandatory: a response
/// without them is a protocol violation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    /// What the change does.
    pub kind: ChangeKind,
    /// Whether the fleet owns the affected resource.
    pub owned: bool,
    /// Whether applying it removes or replaces state.
    pub destructive: bool,
    /// The resource the change affects.
    pub resource: DesiredResource,
}

/// A condition the provider reports instead of failing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Condition {
    /// Condition kind: `auth`, `rate-limited`, `transient`, `conflict`, …
    pub kind: String,
    /// Human-readable detail.
    #[serde(default)]
    pub message: String,
}

/// A recorded plan that can be undone (ADR-030).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackPlan {
    /// Provider the plan belongs to.
    pub provider: String,
    /// Unix time the plan was applied.
    pub applied_at: u64,
    /// Inverse operations, newest last.
    pub changes: Vec<InverseChange>,
}

/// One inverse operation and the state it expects to find.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InverseChange {
    /// The operation that undoes the applied change.
    pub change: Change,
    /// State expected before the rollback. `None` means the resource must be
    /// absent; anything else means it must exist with these attributes.
    #[serde(default)]
    pub expected: Option<DesiredResource>,
}

/// Builds the inverse of an applied plan.
///
/// - a created resource is deleted again;
/// - a deleted resource is created with the attributes it had;
/// - an updated resource is updated back to its previous attributes.
///
pub fn invert(
    applied: &[Change],
    before: &[ObservedResource],
    after: &[ObservedResource],
) -> Vec<InverseChange> {
    let mut inverse = Vec::new();

    for change in applied {
        match change.kind {
            ChangeKind::Create => {
                // The undo needs the resource as the provider now reports it:
                // only the post-apply observation carries the provider's own
                // id, and without it the delete cannot name its target.
                let created = after
                    .iter()
                    .find(|resource| resource.id == change.resource.id)
                    .map(|resource| DesiredResource {
                        kind: resource.kind.clone(),
                        id: resource.id.clone(),
                        attributes: resource.attributes.clone(),
                    })
                    .unwrap_or_else(|| change.resource.clone());
                inverse.push(InverseChange {
                    change: Change {
                        kind: ChangeKind::Delete,
                        owned: change.owned,
                        destructive: true,
                        resource: created.clone(),
                    },
                    expected: Some(created),
                });
            }
            ChangeKind::Update => {
                let Some(previous) = before
                    .iter()
                    .find(|resource| resource.id == change.resource.id)
                else {
                    continue;
                };
                inverse.push(InverseChange {
                    change: Change {
                        kind: ChangeKind::Update,
                        owned: change.owned,
                        destructive: false,
                        resource: DesiredResource {
                            kind: previous.kind.clone(),
                            id: previous.id.clone(),
                            attributes: previous.attributes.clone(),
                        },
                    },
                    expected: Some(change.resource.clone()),
                });
            }
            ChangeKind::Delete => inverse.push(InverseChange {
                change: Change {
                    kind: ChangeKind::Create,
                    owned: change.owned,
                    destructive: false,
                    resource: change.resource.clone(),
                },
                expected: None,
            }),
        }
    }

    inverse
}

/// Whether an observed resource still matches the state a plan expects.
///
/// Only the attributes the expected resource names are compared: providers add
/// bookkeeping fields (record ids, markers) that are not desired state.
pub fn matches_expected(current: &ObservedResource, expected: &DesiredResource) -> bool {
    let Some(want) = expected.attributes.as_object() else {
        return true;
    };
    let Some(have) = current.attributes.as_object() else {
        return false;
    };
    want.iter().all(|(key, value)| have.get(key) == Some(value))
}

#[cfg(test)]
mod rollback_tests {
    use super::*;
    use serde_json::json;

    fn resource(id: &str, value: &str) -> DesiredResource {
        DesiredResource {
            kind: "dns.record".to_string(),
            id: id.to_string(),
            attributes: json!({ "content": value }),
        }
    }

    fn change(kind: ChangeKind, id: &str, value: &str) -> Change {
        Change {
            kind,
            owned: true,
            destructive: matches!(kind, ChangeKind::Delete),
            resource: resource(id, value),
        }
    }

    #[test]
    fn a_create_is_undone_by_a_delete_that_names_the_provider_record() {
        let after = vec![ObservedResource {
            kind: "dns.record".to_string(),
            id: "one".to_string(),
            attributes: json!({ "content": "v1", "record_id": "cf-123" }),
            owned: true,
        }];

        let inverse = invert(&[change(ChangeKind::Create, "one", "v1")], &[], &after);
        assert_eq!(inverse[0].change.kind, ChangeKind::Delete);
        assert_eq!(inverse[0].change.resource.attributes["record_id"], "cf-123");
        assert_eq!(inverse[0].expected.as_ref().unwrap().id, "one");
    }

    #[test]
    fn a_create_without_a_post_apply_observation_still_has_an_undo() {
        let inverse = invert(&[change(ChangeKind::Create, "one", "v1")], &[], &[]);
        assert_eq!(inverse[0].change.kind, ChangeKind::Delete);
        assert_eq!(inverse[0].change.resource.id, "one");
    }

    #[test]
    fn a_delete_is_undone_by_recreating_the_old_attributes() {
        let inverse = invert(&[change(ChangeKind::Delete, "one", "old")], &[], &[]);
        assert_eq!(inverse[0].change.kind, ChangeKind::Create);
        assert_eq!(inverse[0].change.resource.attributes["content"], "old");
        assert!(inverse[0].expected.is_none());
    }

    #[test]
    fn an_update_is_undone_with_the_previous_attributes() {
        let observed = vec![ObservedResource {
            kind: "dns.record".to_string(),
            id: "one".to_string(),
            attributes: json!({ "content": "old" }),
            owned: true,
        }];
        let inverse = invert(&[change(ChangeKind::Update, "one", "new")], &observed, &[]);
        assert_eq!(inverse[0].change.kind, ChangeKind::Update);
        assert_eq!(inverse[0].change.resource.attributes["content"], "old");
        assert_eq!(
            inverse[0].expected.as_ref().unwrap().attributes["content"],
            "new"
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderError {
    /// Error kind: `auth`, `rate-limited`, `transient`, `config`, `protocol`.
    pub kind: String,
    /// Human-readable detail.
    pub message: String,
    /// Seconds to wait, when the provider knows (ADR-030).
    #[serde(default)]
    pub retry_after_secs: Option<u64>,
}

/// Result of one applied change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedChange {
    /// Resource identifier.
    pub id: String,
    /// Whether it was applied.
    pub ok: bool,
    /// Optional detail.
    #[serde(default)]
    pub message: String,
    /// Classified failure, when the provider has one. `message` stays for
    /// providers that predate this field; the core prefers `error`.
    #[serde(default)]
    pub error: Option<ProviderError>,
}

/// `observe` request.
#[derive(Debug, Clone, Serialize)]
pub struct ObserveRequest<'a> {
    /// Operation name.
    pub op: &'static str,
    /// Provider settings, opaque to the core.
    pub config: &'a serde_json::Value,
}

/// `observe` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObserveResponse {
    /// Observed resources.
    #[serde(default)]
    pub resources: Vec<ObservedResource>,
    /// Conditions reported by the provider.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// `plan` request.
#[derive(Debug, Clone, Serialize)]
pub struct PlanRequest<'a> {
    /// Operation name.
    pub op: &'static str,
    /// Provider settings, opaque to the core.
    pub config: &'a serde_json::Value,
    /// What the declaration asks for.
    pub desired: &'a [DesiredResource],
    /// What the provider observed.
    pub observed: &'a [ObservedResource],
}

/// `plan` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlanResponse {
    /// Proposed changes.
    #[serde(default)]
    pub changes: Vec<Change>,
    /// Conditions reported by the provider.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// `apply` request: only changes the core approved.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyRequest<'a> {
    /// Operation name.
    pub op: &'static str,
    /// Provider settings, opaque to the core.
    pub config: &'a serde_json::Value,
    /// Approved changes.
    pub changes: &'a [Change],
}

/// `apply` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApplyResponse {
    /// Per-change results.
    #[serde(default)]
    pub applied: Vec<AppliedChange>,
    /// Conditions reported by the provider.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// `adopt` request: stamp the provider's ownership marker on existing
/// resources that already belong to the declaration.
///
/// Attributes are the **observed** ones, so a provider can address the record
/// it must stamp. The core decides which resources are eligible; the provider
/// only writes its own marker and changes nothing else.
#[derive(Debug, Clone, Serialize)]
pub struct AdoptRequest<'a> {
    /// Operation name.
    pub op: &'static str,
    /// Provider settings, opaque to the core.
    pub config: &'a serde_json::Value,
    /// Resources to mark as owned.
    pub resources: &'a [ObservedResource],
}

/// `adopt` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdoptResponse {
    /// Per-resource results; `ok` means the marker is now present.
    #[serde(default)]
    pub adopted: Vec<AppliedChange>,
    /// Conditions reported by the provider.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_owned_flag_means_unowned() {
        let resource: ObservedResource =
            serde_json::from_str(r#"{"kind":"dns.record","id":"web1"}"#).unwrap();
        assert!(!resource.owned);
    }

    #[test]
    fn ownership_serialises_kebab_case() {
        assert_eq!(
            serde_json::to_string(&OwnershipMode::Namespace).unwrap(),
            "\"namespace\""
        );
    }

    #[test]
    fn a_change_without_destructive_is_invalid() {
        let parsed: Result<Change, _> = serde_json::from_str(
            r#"{"kind":"delete","owned":true,"resource":{"kind":"dns.record","id":"x"}}"#,
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn read_only_ownership_forbids_writes() {
        assert!(!OwnershipMode::None.allows_writes());
        assert!(OwnershipMode::Marker.allows_writes());
    }

    #[test]
    fn provider_order_defaults_to_independent_and_serialises_kebab_case() {
        assert_eq!(ProviderOrder::default(), ProviderOrder::Independent);
        assert_eq!(
            serde_json::to_string(&ProviderOrder::BeforeHosts).unwrap(),
            "\"before-hosts\""
        );
    }

    #[test]
    fn a_manifest_declares_protocol_and_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider.json");
        std::fs::write(
            &path,
            r#"{"name":"example","version":"0.1.0","protocol":1,"ownership":"marker"}"#,
        )
        .unwrap();

        let manifest = Manifest::read(&path).unwrap();
        assert_eq!(manifest.name, "example");
        assert_eq!(manifest.ownership, OwnershipMode::Marker);
        assert!(manifest.capabilities.is_empty());
    }
}
