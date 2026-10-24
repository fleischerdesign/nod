//! What the fleet proved it owns (ADR-037).
//!
//! Three kinds of evidence, one question: may the fleet touch this resource?
//! A provider that has a free-form field answers with its marker. A kind
//! without one cannot, so the provider reports `owned: false` and the core
//! answers from here.
//!
//! Two evidence forms are recorded: the rule that produced a resource
//! (ADR-036), and a resource this fleet created itself. The record is not a
//! declaration — it never asks for anything — it only decides whether what
//! already exists is ours.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One resource, by kind and id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedId {
    /// Resource kind, e.g. `authentik.application`.
    pub kind: String,
    /// Resource id, e.g. `app-ada`.
    pub id: String,
}

impl OwnedId {
    /// Builds an entry.
    pub fn new(kind: &str, id: &str) -> Self {
        Self {
            kind: kind.to_string(),
            id: id.to_string(),
        }
    }
}

/// The resources one provider's declaration proved it owns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ownership {
    /// Provider the record belongs to.
    #[serde(default)]
    pub provider: String,
    /// Rule id -> the resources it produced in the last successful run.
    #[serde(default)]
    pub rules: BTreeMap<String, Vec<OwnedId>>,
    /// Resources this fleet created and no rule derives.
    ///
    /// A marker-less kind cannot say so itself; for a kind with a marker this
    /// list is redundant but harmless, and it is what lets a static
    /// declaration manage such a kind at all.
    #[serde(default)]
    pub created: Vec<OwnedId>,
    /// Resources an operator explicitly took over (ADR-040).
    ///
    /// Adoption of a marker-less kind writes nothing into the object; this
    /// entry is the whole evidence, and it exists only because a human asked
    /// for it. A marker-carrying kind is recorded here as well, so the fleet
    /// keeps one ownership model.
    #[serde(default)]
    pub adopted: Vec<OwnedId>,
}

impl Ownership {
    /// The resources a rule produced; empty for a new or removed rule.
    pub fn rule(&self, id: &str) -> &[OwnedId] {
        self.rules.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether `rule` produced `kind`/`id`.
    pub fn contains(&self, rule: &str, kind: &str, id: &str) -> bool {
        self.rule(rule)
            .iter()
            .any(|known| known.kind == kind && known.id == id)
    }

    /// Whether the record proves ownership of `kind`/`id`, by rule, creation or
    /// an operator's adoption.
    pub fn owns(&self, kind: &str, id: &str) -> bool {
        self.created
            .iter()
            .chain(&self.adopted)
            .chain(self.rules.values().flatten())
            .any(|known| known.kind == kind && known.id == id)
    }

    /// Records a resource this fleet created; idempotent, and never shadows a
    /// rule's provenance.
    pub fn learn(&mut self, kind: &str, id: &str) {
        if self.owns(kind, id) {
            return;
        }
        self.created.push(OwnedId::new(kind, id));
    }

    /// Records an explicit adoption; idempotent, and never shadows a rule's
    /// provenance.
    pub fn adopt(&mut self, kind: &str, id: &str) {
        if self.owns(kind, id) {
            return;
        }
        self.adopted.push(OwnedId::new(kind, id));
    }

    /// Drops `created` entries that are neither observed nor created in this
    /// run.
    ///
    /// A successful create is kept even when the API has not listed it yet:
    /// pruning it would make the resource foreign on the next run, which is the
    /// one state no operator can repair from the declaration. Rule entries are
    /// not touched: they follow the declaration, not the observation, so a dry
    /// run cannot wipe the next run's deletion budget (ADR-036).
    pub fn prune(&mut self, exists: impl Fn(&str, &str) -> bool, keep: &[OwnedId]) {
        let prune = |list: &mut Vec<OwnedId>| {
            list.retain(|entry| {
                exists(&entry.kind, &entry.id)
                    || keep
                        .iter()
                        .any(|fresh| fresh.kind == entry.kind && fresh.id == entry.id)
            });
        };
        prune(&mut self.created);
        prune(&mut self.adopted);
    }

    /// Whether the record knows nothing at all.
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.adopted.is_empty() && self.rules.values().all(Vec::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_answers_which_rule_produced_a_resource() {
        let mut record = Ownership::default();
        record
            .rules
            .insert("per-member".to_string(), vec![OwnedId::new("app", "ada")]);

        assert!(record.contains("per-member", "app", "ada"));
        assert!(record.owns("app", "ada"));
        assert!(!record.owns("app", "zoe"));
        assert!(!record.owns("other", "ada"));
        assert!(!record.is_empty());
    }

    #[test]
    fn ownership_is_scoped_to_the_kind() {
        let mut record = Ownership::default();
        record
            .rules
            .insert("per-member".to_string(), vec![OwnedId::new("app", "ada")]);
        assert!(!record.owns("group", "ada"));
    }

    #[test]
    fn a_created_resource_is_owned_without_a_rule() {
        let mut record = Ownership::default();
        record.learn("app", "static");
        assert!(record.owns("app", "static"));
        assert!(record.rule("per-member").is_empty());
    }

    #[test]
    fn learning_is_idempotent_and_keeps_a_rules_provenance() {
        let mut record = Ownership::default();
        record
            .rules
            .insert("per-member".to_string(), vec![OwnedId::new("app", "ada")]);
        record.learn("app", "ada");
        record.learn("app", "ada");
        assert!(record.created.is_empty(), "a rule already owns it");
    }

    #[test]
    fn an_adopted_resource_is_owned_without_a_marker_or_a_rule() {
        let mut record = Ownership::default();
        record.adopt("app", "bazarr");
        record.adopt("app", "bazarr");
        assert!(record.owns("app", "bazarr"));
        assert_eq!(record.adopted.len(), 1, "adoption is idempotent");
        assert!(record.created.is_empty());
    }

    #[test]
    fn pruning_drops_what_is_gone_and_keeps_a_fresh_create() {
        let mut record = Ownership::default();
        record.learn("app", "gone");
        record.learn("app", "fresh");
        record.learn("app", "observed");
        record.rules.insert(
            "per-member".to_string(),
            vec![OwnedId::new("app", "planned")],
        );

        record.prune(
            |_kind, id| id == "observed",
            &[OwnedId::new("app", "fresh")],
        );

        assert!(!record.owns("app", "gone"));
        assert!(record.owns("app", "fresh"), "a create not yet listed stays");
        assert!(record.owns("app", "observed"));
        assert!(
            record.owns("app", "planned"),
            "a rule's budget follows the declaration, not the observation"
        );
    }

    #[test]
    fn an_empty_record_is_empty_even_with_rule_keys() {
        let mut record = Ownership::default();
        record.rules.insert("per-member".to_string(), Vec::new());
        assert!(record.is_empty());
    }
}
