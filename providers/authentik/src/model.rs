//! Pure mapping and planning for the Authentik groups provider.
//!
//! The protocol and the HTTP client are I/O; this module is neither. It maps
//! Authentik groups to nod resources and computes the plan, so the rules can be
//! tested without a token, an instance or a network.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Resource kind for an Authentik group.
pub const KIND_GROUP: &str = "authentik.group";

/// A group as Authentik returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    /// Unique group name; the provider's stable id.
    pub name: String,
    /// Whether members become superusers.
    #[serde(default)]
    pub is_superuser: bool,
    /// Free-form attributes; the ownership marker lives here.
    #[serde(default)]
    pub attributes: Value,
}

/// The ownership marker, configured per provider instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// Attribute key, e.g. `nod`.
    pub key: String,
    /// Attribute value, e.g. `owned`.
    pub value: String,
}

impl Marker {
    /// Whether `group` carries this marker.
    pub fn matches(&self, group: &Group) -> bool {
        group.attributes.get(&self.key).and_then(Value::as_str) == Some(self.value.as_str())
    }
}

/// A nod resource, as the protocol defines it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resource {
    /// Resource kind, e.g. `authentik.group`.
    pub kind: String,
    /// Provider-scoped identifier; the group name.
    pub id: String,
    /// Provider-specific attributes.
    #[serde(default)]
    pub attributes: Value,
}

/// An observed resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservedResource {
    /// Resource kind.
    pub kind: String,
    /// Provider-scoped identifier.
    pub id: String,
    /// Provider-specific attributes.
    #[serde(default)]
    pub attributes: Value,
    /// Whether the fleet owns it.
    #[serde(default)]
    pub owned: bool,
}

/// One proposed change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    /// `create`, `update` or `delete`.
    pub kind: String,
    /// Whether the fleet owns the target.
    pub owned: bool,
    /// Whether the change removes something.
    pub destructive: bool,
    /// The resource the change addresses.
    pub resource: Resource,
}

/// Maps a group to the resource the core sees.
///
/// Only fields the declaration controls are exposed: the name is the id, and
/// `is_superuser` is the one attribute. The free-form `attributes` object stays
/// out of desired state because the ownership marker lives there.
pub fn to_resource(group: &Group, marker: &Marker) -> ObservedResource {
    ObservedResource {
        kind: KIND_GROUP.to_string(),
        id: group.name.clone(),
        attributes: json!({ "is_superuser": group.is_superuser }),
        owned: marker.matches(group),
    }
}

/// Computes the plan from a declaration and an observation.
///
/// A foreign resource is never updated or deleted: adoption is an explicit
/// operation, not something a reconcile invents (ADR-032).
pub fn plan(desired: &[Resource], observed: &[ObservedResource]) -> Vec<Change> {
    let mut changes = Vec::new();

    for want in desired
        .iter()
        .filter(|resource| resource.kind == KIND_GROUP)
    {
        match observed
            .iter()
            .find(|current| current.kind == want.kind && current.id == want.id)
        {
            None => changes.push(Change {
                kind: "create".to_string(),
                owned: true,
                destructive: false,
                resource: want.clone(),
            }),
            Some(current) if !current.owned => {}
            Some(current) if current.attributes != want.attributes => changes.push(Change {
                kind: "update".to_string(),
                owned: true,
                destructive: false,
                resource: want.clone(),
            }),
            Some(_) => {}
        }
    }

    for current in observed
        .iter()
        .filter(|resource| resource.kind == KIND_GROUP && resource.owned)
    {
        let declared = desired
            .iter()
            .any(|want| want.kind == current.kind && want.id == current.id);
        if !declared {
            changes.push(Change {
                kind: "delete".to_string(),
                owned: true,
                destructive: true,
                resource: Resource {
                    kind: current.kind.clone(),
                    id: current.id.clone(),
                    attributes: current.attributes.clone(),
                },
            });
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker() -> Marker {
        Marker {
            key: "nod".to_string(),
            value: "owned".to_string(),
        }
    }

    fn group(name: &str, superuser: bool, attributes: Value) -> Group {
        Group {
            name: name.to_string(),
            is_superuser: superuser,
            attributes,
        }
    }

    fn resource(name: &str, superuser: bool) -> Resource {
        Resource {
            kind: KIND_GROUP.to_string(),
            id: name.to_string(),
            attributes: json!({ "is_superuser": superuser }),
        }
    }

    #[test]
    fn the_marker_is_read_from_attributes() {
        assert!(marker().matches(&group("admins", false, json!({ "nod": "owned" }))));
        assert!(!marker().matches(&group("users", false, json!({ "other": "yes" }))));
        assert!(!marker().matches(&group("empty", false, json!({}))));
    }

    #[test]
    fn a_group_maps_name_to_id_and_superuser_to_attributes() {
        let mapped = to_resource(&group("admins", true, json!({})), &marker());
        assert_eq!(mapped.kind, KIND_GROUP);
        assert_eq!(mapped.id, "admins");
        assert_eq!(mapped.attributes, json!({ "is_superuser": true }));
        assert!(!mapped.owned);
    }

    #[test]
    fn a_missing_group_is_created() {
        let changes = plan(&[resource("admins", false)], &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "create");
        assert!(changes[0].owned);
        assert!(!changes[0].destructive);
    }

    #[test]
    fn an_owned_group_with_different_attributes_is_updated() {
        let observed = vec![to_resource(
            &group("admins", false, json!({ "nod": "owned" })),
            &marker(),
        )];
        let changes = plan(&[resource("admins", true)], &observed);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "update");
    }

    #[test]
    fn an_owned_group_with_equal_attributes_is_unchanged() {
        let observed = vec![to_resource(
            &group("admins", true, json!({ "nod": "owned" })),
            &marker(),
        )];
        assert!(plan(&[resource("admins", true)], &observed).is_empty());
    }

    #[test]
    fn a_foreign_group_is_never_updated() {
        let observed = vec![to_resource(&group("admins", false, json!({})), &marker())];
        assert!(plan(&[resource("admins", true)], &observed).is_empty());
    }

    #[test]
    fn an_owned_group_outside_the_declaration_is_deleted() {
        let observed = vec![to_resource(
            &group("old", false, json!({ "nod": "owned" })),
            &marker(),
        )];
        let changes = plan(&[], &observed);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "delete");
        assert!(changes[0].destructive);
    }

    #[test]
    fn a_foreign_group_outside_the_declaration_is_left_alone() {
        let observed = vec![to_resource(&group("foreign", false, json!({})), &marker())];
        assert!(plan(&[], &observed).is_empty());
    }

    #[test]
    fn other_kinds_are_ignored() {
        let desired = vec![Resource {
            kind: "authentik.application".to_string(),
            id: "app".to_string(),
            attributes: json!({}),
        }];
        assert!(plan(&desired, &[]).is_empty());
    }
}
