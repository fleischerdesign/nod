//! Pure mapping and planning for the Cloudflare DNS provider.
//!
//! The protocol and the HTTP client are I/O; this module is neither. It maps
//! Cloudflare records to nod resources and computes the plan, so the rules can
//! be tested without a token, a zone or a network.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Marker prefix in a record comment. A record is ours when its comment starts
/// with this, and only then may it be changed destructively.
pub const MARKER: &str = "nod:";

/// A DNS record as Cloudflare returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct DnsRecord {
    /// Cloudflare record id.
    pub id: String,
    /// Record type, e.g. `A`.
    #[serde(rename = "type")]
    pub record_type: String,
    /// Record name.
    pub name: String,
    /// Record content.
    pub content: String,
    /// TTL in seconds.
    #[serde(default)]
    pub ttl: u32,
    /// Whether the record is proxied.
    #[serde(default)]
    pub proxied: bool,
    /// Free-form comment, used as the ownership marker.
    #[serde(default)]
    pub comment: Option<String>,
}

/// A nod resource, as the protocol defines it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resource {
    /// Always `dns.record`.
    pub kind: String,
    /// `TYPE:name`, stable within the zone.
    pub id: String,
    /// Provider attributes.
    #[serde(default)]
    pub attributes: Value,
}

/// Resource id for a record.
pub fn resource_id(record_type: &str, name: &str) -> String {
    format!("{record_type}:{name}")
}

/// Whether a record carries our marker.
pub fn is_owned(record: &DnsRecord) -> bool {
    record
        .comment
        .as_deref()
        .is_some_and(|comment| comment.starts_with(MARKER))
}

/// Maps a record to a resource; `owned` is set from the marker.
pub fn to_resource(record: &DnsRecord) -> (Resource, bool) {
    let resource = Resource {
        kind: "dns.record".to_string(),
        id: resource_id(&record.record_type, &record.name),
        attributes: json!({
            "type": record.record_type,
            "name": record.name,
            "content": record.content,
            "ttl": record.ttl,
            "proxied": record.proxied,
            "comment": record.comment,
            "record_id": record.id,
        }),
    };
    (resource, is_owned(record))
}

/// A change, as the protocol defines it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Change {
    /// `create`, `update` or `delete`.
    pub kind: &'static str,
    /// Whether the fleet owns the resource.
    pub owned: bool,
    /// Whether the change removes or replaces state.
    pub destructive: bool,
    /// The affected resource.
    pub resource: Resource,
}

/// What a desired entry asks for.
#[derive(Debug, Clone, Deserialize)]
pub struct Desired {
    /// Resource kind; only `dns.record` is accepted.
    pub kind: String,
    /// `TYPE:name`.
    pub id: String,
    /// Attributes: type, name, content, ttl, proxied.
    #[serde(default)]
    pub attributes: Value,
}

/// Computes the plan by comparing the declaration with the observed records.
///
/// Rules:
/// - absent from the zone → `create` (owned)
/// - present, ours, attributes differ → `update`
/// - present, foreign, attributes differ → `update` with `owned = false`, so
///   the core refuses it and reports the conflict
/// - present, ours, absent from the declaration → `delete` (destructive)
/// - present, foreign, absent from the declaration → nothing: we do not delete
///   what we do not own, and we do not even propose it
pub fn plan(desired: &[Desired], observed: &[(Resource, bool)]) -> Vec<Change> {
    let mut changes = Vec::new();

    for want in desired {
        let existing = observed.iter().find(|(resource, _)| resource.id == want.id);

        match existing {
            None => changes.push(Change {
                kind: "create",
                owned: true,
                destructive: false,
                resource: Resource {
                    kind: want.kind.clone(),
                    id: want.id.clone(),
                    attributes: want.attributes.clone(),
                },
            }),
            Some((resource, owned)) => {
                if attributes_match(resource, want) {
                    continue;
                }
                changes.push(Change {
                    kind: "update",
                    owned: *owned,
                    destructive: false,
                    resource: Resource {
                        kind: want.kind.clone(),
                        id: want.id.clone(),
                        attributes: want.attributes.clone(),
                    },
                });
            }
        }
    }

    for (resource, owned) in observed {
        let wanted = desired.iter().any(|want| want.id == resource.id);
        if !wanted && *owned {
            changes.push(Change {
                kind: "delete",
                owned: true,
                destructive: true,
                resource: resource.clone(),
            });
        }
    }

    changes
}

/// Whether the observed attributes already satisfy the desired ones.
///
/// Only the fields a declaration controls are compared: `record_id` and
/// `comment` are provider bookkeeping, not desired state.
fn attributes_match(observed: &Resource, desired: &Desired) -> bool {
    let fields = ["type", "name", "content", "ttl", "proxied"];
    fields.iter().all(|field| {
        let want = desired.attributes.get(field);
        match want {
            None => true,
            Some(want) => observed.attributes.get(field) == Some(want),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, content: &str, comment: Option<&str>) -> DnsRecord {
        DnsRecord {
            id: format!("id-{name}"),
            record_type: "A".to_string(),
            name: name.to_string(),
            content: content.to_string(),
            ttl: 1,
            proxied: false,
            comment: comment.map(str::to_string),
        }
    }

    fn desired(name: &str, content: &str) -> Desired {
        Desired {
            kind: "dns.record".to_string(),
            id: resource_id("A", name),
            attributes: json!({"type":"A","name":name,"content":content,"ttl":1,"proxied":false}),
        }
    }

    #[test]
    fn a_record_without_the_marker_is_foreign() {
        let (_, owned) = to_resource(&record("www", "1.2.3.4", None));
        assert!(!owned);

        let (_, owned) = to_resource(&record("www", "1.2.3.4", Some("nod:vyrx")));
        assert!(owned);
    }

    #[test]
    fn a_missing_record_is_created() {
        let changes = plan(&[desired("www", "1.2.3.4")], &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "create");
        assert!(changes[0].owned);
    }

    #[test]
    fn an_unchanged_owned_record_is_left_alone() {
        let observed = vec![to_resource(&record("www", "1.2.3.4", Some("nod:vyrx")))];
        assert!(plan(&[desired("www", "1.2.3.4")], &observed).is_empty());
    }

    #[test]
    fn a_changed_owned_record_is_updated() {
        let observed = vec![to_resource(&record("www", "1.2.3.4", Some("nod:vyrx")))];
        let changes = plan(&[desired("www", "5.6.7.8")], &observed);
        assert_eq!(changes[0].kind, "update");
        assert!(changes[0].owned);
    }

    #[test]
    fn a_foreign_record_that_differs_is_proposed_as_unowned() {
        let observed = vec![to_resource(&record("www", "1.2.3.4", None))];
        let changes = plan(&[desired("www", "5.6.7.8")], &observed);
        assert_eq!(changes[0].kind, "update");
        assert!(!changes[0].owned);
    }

    #[test]
    fn a_removed_owned_record_is_a_destructive_delete() {
        let observed = vec![to_resource(&record("old", "1.2.3.4", Some("nod:vyrx")))];
        let changes = plan(&[], &observed);
        assert_eq!(changes[0].kind, "delete");
        assert!(changes[0].destructive);
    }

    #[test]
    fn a_removed_foreign_record_is_not_proposed_at_all() {
        let observed = vec![to_resource(&record("old", "1.2.3.4", None))];
        assert!(plan(&[], &observed).is_empty());
    }
}
