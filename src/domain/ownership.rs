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

use crate::domain::errors::NodError;

/// Persists an ownership record between two steps of a reconcile run.
///
/// The run cannot write the record itself - the store lives outside the use
/// case - but a write-ahead has to happen while the run is in flight: the
/// intent is recorded before the provider is asked to create anything, so a
/// crash in between leaves evidence instead of a resource nobody claims.
///
/// A sink is called at most twice per run: once with the resolved record
/// (after the observation, when an earlier attempt's intent can be settled)
/// and once with the record a create is about to produce. A failed persist is
/// a configuration error and aborts the run - continuing would perform writes
/// the record does not back.
pub trait OwnershipSink {
    /// Writes `record`, replacing the provider's previous record.
    fn persist(&self, record: &Ownership) -> Result<(), NodError>;
}

/// The sink for runs that observe without writing.
///
/// `provider status` and dry runs resolve a pending intent in memory so their
/// report is truthful, but they must not touch the record: a read-only command
/// that rewrites state is not read-only.
pub struct NoopSink;

impl OwnershipSink for NoopSink {
    fn persist(&self, _record: &Ownership) -> Result<(), NodError> {
        Ok(())
    }
}

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
    /// Resources a run was about to create when it wrote this record.
    ///
    /// This is the write-ahead: a create is recorded *before* the provider is
    /// asked to perform it, so a crash between apply and the final record
    /// leaves evidence instead of a foreign resource. A pending entry is not
    /// ownership on its own - `owns` ignores it, because a create that never
    /// happened must not let a later run claim a resource that happens to
    /// carry the id. `resolve_pending` promotes it once an observation
    /// confirms the resource exists, and drops it otherwise.
    ///
    /// Skipped when empty so the field does not appear in records that have
    /// nothing pending: an older reader refuses unknown fields, and that
    /// refusal would turn a readable record into "everything is foreign".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<OwnedId>,
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

    /// Records what this run is about to create, replacing any previous plan.
    ///
    /// Replacing rather than appending is deliberate: a pending entry belongs
    /// to one attempt. An earlier attempt whose outcome is unknown was already
    /// resolved by `resolve_pending` before this run planned anything.
    pub fn mark_pending(&mut self, ids: &[OwnedId]) {
        self.pending = ids
            .iter()
            .filter(|entry| !self.owns(&entry.kind, &entry.id))
            .cloned()
            .collect();
    }

    /// Turns every pending create the observation confirms into a creation, and
    /// forgets the rest.
    ///
    /// Run this before `mark_owned`: it is the only place where an interrupted
    /// run's intent becomes evidence. A pending entry the provider does not
    /// report never happened - a failed apply, a declined provider, a crash
    /// before the call - and keeping it would be a claim without a resource.
    pub fn resolve_pending(&mut self, exists: impl Fn(&str, &str) -> bool) {
        let mut confirmed: Vec<OwnedId> = Vec::new();
        for entry in std::mem::take(&mut self.pending) {
            if exists(&entry.kind, &entry.id) {
                confirmed.push(entry);
            }
        }
        for entry in confirmed {
            if !self.owns(&entry.kind, &entry.id) {
                self.created.push(entry);
            }
        }
    }

    /// Whether the record knows nothing at all.
    pub fn is_empty(&self) -> bool {
        self.created.is_empty()
            && self.adopted.is_empty()
            && self.pending.is_empty()
            && self.rules.values().all(Vec::is_empty)
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

    #[test]
    fn a_pending_create_is_not_ownership_before_it_is_resolved() {
        let mut record = Ownership::default();
        record.mark_pending(&[OwnedId::new("authentik.certificate_keypair", "OIDC signing key")]);

        assert!(
            !record.owns("authentik.certificate_keypair", "OIDC signing key"),
            "an intent is not evidence"
        );
        assert!(!record.is_empty(), "a pending create is worth writing");
    }

    #[test]
    fn a_pending_create_becomes_a_creation_once_the_provider_reports_it() {
        let mut record = Ownership::default();
        record.mark_pending(&[OwnedId::new("kind", "made")]);

        record.resolve_pending(|_kind, id| id == "made");

        assert!(record.owns("kind", "made"));
        assert_eq!(record.created.len(), 1);
        assert!(record.pending.is_empty(), "the intent is resolved, not kept");
    }

    #[test]
    fn a_pending_create_that_never_happened_is_forgotten() {
        let mut record = Ownership::default();
        record.mark_pending(&[OwnedId::new("kind", "never-created")]);

        record.resolve_pending(|_kind, _id| false);

        assert!(
            !record.owns("kind", "never-created"),
            "a create that did not happen must not claim a foreign resource"
        );
        assert!(record.pending.is_empty());
    }

    #[test]
    fn resolving_a_pending_create_a_rule_already_owns_keeps_the_rules_provenance() {
        let mut record = Ownership::default();
        record
            .rules
            .insert("per-member".to_string(), vec![OwnedId::new("app", "ada")]);

        record.mark_pending(&[OwnedId::new("app", "ada")]);

        assert!(
            record.pending.is_empty(),
            "a rule already owns it, so nothing is pending"
        );
        assert!(record.created.is_empty(), "the rule keeps its provenance");
    }

    #[test]
    fn a_record_without_pending_does_not_carry_the_field() {
        let raw = serde_json::to_string(&Ownership::default()).unwrap();
        assert!(
            !raw.contains("pending"),
            "an older reader refuses unknown fields; the empty case must not appear"
        );

        let mut record = Ownership::default();
        record.mark_pending(&[OwnedId::new("kind", "made")]);
        let raw = serde_json::to_string(&record).unwrap();
        assert!(raw.contains("pending"));
        let back: Ownership = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.pending.len(), 1);
    }
}
