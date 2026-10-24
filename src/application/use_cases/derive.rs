//! Declared derivations (ADR-036).
//!
//! A rule expands a template over one attribute of one observed resource. The
//! expansion is a pure function of a successful observation: an unavailable
//! source is a configuration error, never an empty set.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::domain::errors::NodError;
use crate::domain::provider::{DesiredResource, ObservedResource};

/// The desired-state schema this core understands.
///
/// `derive` was added in schema 2; a file that declares a newer schema is
/// refused instead of being silently misread.
pub const SCHEMA: u64 = 2;

/// One declared rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeriveRule {
    /// Stable rule id, used for provenance and the deletion budget.
    pub id: String,
    /// The observed resource and attribute the set follows.
    #[serde(rename = "from")]
    pub from: Source,
    /// What to materialise per member.
    pub template: Template,
    /// How many derived removals this rule may cause in one run.
    ///
    /// `0` freezes the set: a member that leaves is not deleted until the
    /// operator allows it. The default is deliberately the safe one.
    #[serde(default)]
    pub max_deletions: usize,
}

/// Where a rule reads its members.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Observed resource kind.
    pub kind: String,
    /// Observed resource id.
    pub id: String,
    /// Attribute holding the member list.
    pub attribute: String,
}

/// What a rule materialises per member.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// Resource kind to create.
    pub kind: String,
    /// Resource id, with `{member}` substituted.
    pub id: String,
    /// Resource attributes, with `{member}` substituted in strings.
    #[serde(default)]
    pub attributes: Value,
}

/// One expanded resource and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    /// The resource the provider will see.
    pub resource: DesiredResource,
    /// Rule that produced it.
    pub rule: String,
    /// Member that produced it.
    pub member: String,
}

/// Parses the `derive` list of a desired-state document.
pub fn parse(value: &Value) -> Result<Vec<DeriveRule>, NodError> {
    serde_json::from_value(value.clone())
        .map_err(|error| NodError::config(format!("invalid derivation rule: {error}")))
}

/// Expands every rule against one successful observation.
///
/// `static_resources` are the declaration's own list: a derived id that
/// collides with one of them is a configuration error, not a merge.
pub fn expand(
    rules: &[DeriveRule],
    observed: &[ObservedResource],
    static_resources: &[DesiredResource],
) -> Result<Vec<Derived>, NodError> {
    let mut derived = Vec::new();
    let mut seen: BTreeMap<(String, String), String> = BTreeMap::new();

    for resource in static_resources {
        seen.insert(
            (resource.kind.clone(), resource.id.clone()),
            "the declaration".to_string(),
        );
    }

    for rule in rules {
        let source = observed
            .iter()
            .find(|resource| resource.kind == rule.from.kind && resource.id == rule.from.id)
            .ok_or_else(|| {
                NodError::config(format!(
                    "derivation `{}` names source {} {}, which the provider did not report",
                    rule.id, rule.from.kind, rule.from.id
                ))
            })?;

        let value = source.attributes.get(&rule.from.attribute).ok_or_else(|| {
            NodError::config(format!(
                "derivation `{}` reads `{}` from {} {}, but the attribute is missing",
                rule.id, rule.from.attribute, rule.from.kind, rule.from.id
            ))
        })?;
        let members = value.as_array().ok_or_else(|| {
            NodError::config(format!(
                "derivation `{}` reads `{}` from {} {}, but it is not a list",
                rule.id, rule.from.attribute, rule.from.kind, rule.from.id
            ))
        })?;

        let mut rendered = Vec::new();
        for member in members {
            let member = member.as_str().ok_or_else(|| {
                NodError::config(format!(
                    "derivation `{}` found a non-string member in {} {}",
                    rule.id, rule.from.kind, rule.from.id
                ))
            })?;
            if member.is_empty() {
                return Err(NodError::config(format!(
                    "derivation `{}` found an empty member in {} {}",
                    rule.id, rule.from.kind, rule.from.id
                )));
            }

            let id = rule.template.id.replace("{member}", member);
            let key = (rule.template.kind.clone(), id.clone());
            if let Some(producer) = seen.get(&key) {
                return Err(NodError::config(format!(
                    "derivation `{}` produces {} {id}, which {producer} already produces",
                    rule.id, rule.template.kind
                )));
            }
            seen.insert(key, format!("derivation `{}`", rule.id));

            rendered.push(Derived {
                resource: DesiredResource {
                    kind: rule.template.kind.clone(),
                    id,
                    attributes: render(&rule.template.attributes, member),
                },
                rule: rule.id.clone(),
                member: member.to_string(),
            });
        }

        // Deterministic order independent of the source's own ordering.
        rendered.sort_by(|left, right| left.resource.id.cmp(&right.resource.id));
        derived.extend(rendered);
    }

    Ok(derived)
}

/// Substitutes `{member}` in every string of a JSON value.
fn render(value: &Value, member: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("{member}", member)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| render(item, member)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), render(item, member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn observed_group(members: Value) -> ObservedResource {
        ObservedResource {
            kind: "authentik.group".to_string(),
            id: "ai-users".to_string(),
            attributes: json!({ "users": members }),
            owned: true,
        }
    }

    fn rule(max_deletions: usize) -> DeriveRule {
        DeriveRule {
            id: "openclaw-per-member".to_string(),
            from: Source {
                kind: "authentik.group".to_string(),
                id: "ai-users".to_string(),
                attribute: "users".to_string(),
            },
            template: Template {
                kind: "openclaw.instance".to_string(),
                id: "{member}".to_string(),
                attributes: json!({ "owner": "{member}", "group": "ai-users" }),
            },
            max_deletions,
        }
    }

    #[test]
    fn a_rule_expands_one_resource_per_member() {
        let derived = expand(&[rule(0)], &[observed_group(json!(["zoe", "ada"]))], &[])
            .expect("the source exists");

        assert_eq!(derived.len(), 2);
        assert_eq!(derived[0].resource.id, "ada", "members are sorted");
        assert_eq!(derived[0].resource.attributes["owner"], json!("ada"));
        assert_eq!(derived[1].resource.id, "zoe");
        assert_eq!(derived[0].rule, "openclaw-per-member");
        assert_eq!(derived[0].member, "ada");
    }

    #[test]
    fn a_missing_source_is_an_error_not_an_empty_set() {
        let error = expand(&[rule(0)], &[], &[]).expect_err("a missing source must fail");
        assert!(error.to_string().contains("did not report"), "{error}");
    }

    #[test]
    fn a_missing_attribute_is_an_error() {
        let source = ObservedResource {
            kind: "authentik.group".to_string(),
            id: "ai-users".to_string(),
            attributes: json!({}),
            owned: true,
        };
        let error = expand(&[rule(0)], &[source], &[]).expect_err("a missing attribute must fail");
        assert!(
            error.to_string().contains("attribute is missing"),
            "{error}"
        );
    }

    #[test]
    fn a_non_list_attribute_is_an_error() {
        let error = expand(&[rule(0)], &[observed_group(json!("ada"))], &[])
            .expect_err("a non-list attribute must fail");
        assert!(error.to_string().contains("not a list"), "{error}");
    }

    #[test]
    fn a_non_string_member_is_an_error() {
        let error = expand(&[rule(0)], &[observed_group(json!(["ada", 7]))], &[])
            .expect_err("a non-string member must fail");
        assert!(error.to_string().contains("non-string member"), "{error}");
    }

    #[test]
    fn an_empty_source_produces_an_empty_set() {
        let derived = expand(&[rule(0)], &[observed_group(json!([]))], &[])
            .expect("an empty group is a fact, not a failure");
        assert!(derived.is_empty());
    }

    #[test]
    fn two_rules_producing_the_same_id_are_an_error() {
        let error = expand(&[rule(0), rule(0)], &[observed_group(json!(["ada"]))], &[])
            .expect_err("a duplicate id must fail");
        assert!(error.to_string().contains("already produces"), "{error}");
    }

    #[test]
    fn a_derived_id_colliding_with_a_static_resource_is_an_error() {
        let static_resource = DesiredResource {
            kind: "openclaw.instance".to_string(),
            id: "ada".to_string(),
            attributes: json!({}),
        };
        let error = expand(
            &[rule(0)],
            &[observed_group(json!(["ada"]))],
            &[static_resource],
        )
        .expect_err("a collision with the declaration must fail");
        assert!(error.to_string().contains("the declaration"), "{error}");
    }

    #[test]
    fn a_rule_parses_and_rejects_unknown_fields() {
        let value = json!([{
            "id": "rule",
            "from": {"kind": "k", "id": "i", "attribute": "a"},
            "template": {"kind": "t", "id": "{member}"}
        }]);
        assert_eq!(parse(&value).expect("a valid rule").len(), 1);

        let typo = json!([{
            "id": "rule",
            "form": {"kind": "k", "id": "i", "attribute": "a"},
            "template": {"kind": "t", "id": "{member}"}
        }]);
        assert!(parse(&typo).is_err(), "a typo must be loud");
    }
}
