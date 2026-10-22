//! Adoption of existing resources (ADR-032).
//!
//! A zone that existed before nod is the normal case, not a migration edge.
//! The core decides which observed resources may be marked as owned; the
//! provider only writes its own marker. Nothing is inferred from comments,
//! labels or names: the declaration and the observed attributes decide.

use crate::domain::provider::{matches_expected, DesiredResource, ObservedResource};

/// What the core decided about one observed resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Already carries the provider's marker; nothing to do.
    AlreadyOwned,
    /// Declared and identical: safe to stamp.
    Eligible,
    /// Declared but different: only an explicit `--force` may take it over.
    Conflict {
        /// `key: have != want` for the keys the declaration names.
        differences: Vec<String>,
    },
    /// Not declared: never touched.
    Undeclared,
}

/// One observed resource with the decision about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The observed resource, as the provider reported it.
    pub resource: ObservedResource,
    /// What the core decided.
    pub decision: Decision,
}

/// Differences between an observed resource and the declaration.
///
/// Only the keys the declaration names are compared: providers add bookkeeping
/// fields (`record_id`, markers) that are not desired state.
pub fn differences(current: &ObservedResource, want: &DesiredResource) -> Vec<String> {
    let Some(wanted) = want.attributes.as_object() else {
        return Vec::new();
    };
    let observed = current.attributes.as_object();

    let mut differences = Vec::new();
    for (key, value) in wanted {
        let have = observed.and_then(|attributes| attributes.get(key));
        if have != Some(value) {
            differences.push(format!(
                "{key}: {} != {}",
                have.map_or_else(|| "absent".to_string(), |value| value.to_string()),
                value
            ));
        }
    }
    differences
}

/// Decides what may happen to every observed resource.
///
/// Order matters: ownership first, then the declaration, then the comparison.
pub fn decide(observed: &[ObservedResource], desired: &[DesiredResource]) -> Vec<Candidate> {
    observed
        .iter()
        .map(|resource| {
            let decision = if resource.owned {
                Decision::AlreadyOwned
            } else {
                match desired.iter().find(|want| want.id == resource.id) {
                    None => Decision::Undeclared,
                    Some(want) if matches_expected(resource, want) => Decision::Eligible,
                    Some(want) => Decision::Conflict {
                        differences: differences(resource, want),
                    },
                }
            };
            Candidate {
                resource: resource.clone(),
                decision,
            }
        })
        .collect()
}

/// Declared resources that exist and match but do not carry the marker.
///
/// This is the number an operator needs during onboarding: `0` means the zone
/// is both correct and owned; anything else means drift correction is not
/// active for those records yet.
pub fn unowned(observed: &[ObservedResource], desired: &[DesiredResource]) -> usize {
    observed
        .iter()
        .filter(|resource| {
            !resource.owned
                && desired
                    .iter()
                    .any(|want| want.id == resource.id && matches_expected(resource, want))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn observed(id: &str, content: &str, owned: bool) -> ObservedResource {
        ObservedResource {
            kind: "dns.record".to_string(),
            id: id.to_string(),
            attributes: json!({ "content": content, "ttl": 1, "record_id": "cf-1" }),
            owned,
        }
    }

    fn desired(id: &str, content: &str) -> DesiredResource {
        DesiredResource {
            kind: "dns.record".to_string(),
            id: id.to_string(),
            attributes: json!({ "content": content, "ttl": 1 }),
        }
    }

    #[test]
    fn an_owned_matching_resource_is_left_alone() {
        let candidates = decide(
            &[observed("A:web", "1.2.3.4", true)],
            &[desired("A:web", "1.2.3.4")],
        );
        assert_eq!(candidates[0].decision, Decision::AlreadyOwned);
    }

    #[test]
    fn a_declared_matching_foreign_resource_is_eligible() {
        let candidates = decide(
            &[observed("A:web", "1.2.3.4", false)],
            &[desired("A:web", "1.2.3.4")],
        );
        assert_eq!(candidates[0].decision, Decision::Eligible);
    }

    #[test]
    fn a_declared_different_resource_is_a_conflict_with_its_diff() {
        let candidates = decide(
            &[observed("A:web", "1.2.3.4", false)],
            &[desired("A:web", "5.6.7.8")],
        );
        let Decision::Conflict { differences } = &candidates[0].decision else {
            panic!("expected a conflict, got {:?}", candidates[0].decision);
        };
        assert_eq!(differences.len(), 1);
        assert!(differences[0].contains("content"));
        assert!(differences[0].contains("1.2.3.4"));
        assert!(differences[0].contains("5.6.7.8"));
    }

    #[test]
    fn an_undeclared_resource_is_never_touched() {
        let candidates = decide(&[observed("A:other", "1.2.3.4", false)], &[]);
        assert_eq!(candidates[0].decision, Decision::Undeclared);
    }

    #[test]
    fn unowned_counts_only_declared_matching_foreign_resources() {
        let observed = vec![
            observed("A:web", "1.2.3.4", false),
            observed("A:owned", "1.2.3.4", true),
            observed("A:other", "9.9.9.9", false),
            observed("A:drift", "1.1.1.1", false),
        ];
        let desired = vec![
            desired("A:web", "1.2.3.4"),
            desired("A:owned", "1.2.3.4"),
            desired("A:drift", "2.2.2.2"),
        ];
        assert_eq!(unowned(&observed, &desired), 1);
    }

    #[test]
    fn differences_ignore_bookkeeping_fields() {
        let current = observed("A:web", "1.2.3.4", false);
        let want = desired("A:web", "1.2.3.4");
        assert!(differences(&current, &want).is_empty());
    }
}
