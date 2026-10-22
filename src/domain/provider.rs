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
    /// Declared ownership mode.
    pub ownership: OwnershipMode,
    /// What the provider manages, in one sentence.
    #[serde(default)]
    pub description: Option<String>,
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

/// A failed operation.
///
/// An error is not an empty answer: a provider that cannot authenticate must
/// say so, because `resources: []` would read as "the zone is empty" and turn
/// an unobserved world into a healthy one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderError {
    /// Error kind: `auth`, `rate-limited`, `transient`, `config`, `protocol`.
    pub kind: String,
    /// Human-readable detail.
    pub message: String,
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
