//! What a declaration derived in its last successful run (ADR-036).
//!
//! The record is not a second declaration: it tells the core which owned
//! resources a rule produced, so a later delete can be classified as derived —
//! authorized by the rule and bounded by its budget — instead of implicit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The derived resources of one provider, grouped by rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derivations {
    /// Provider the record belongs to.
    #[serde(default)]
    pub provider: String,
    /// Rule id -> the ids it produced in the last successful run.
    #[serde(default)]
    pub rules: BTreeMap<String, Vec<String>>,
}

impl Derivations {
    /// The ids a rule produced; empty for a new or removed rule.
    pub fn rule(&self, id: &str) -> &[String] {
        self.rules.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether `id` was produced by `rule`.
    pub fn contains(&self, rule: &str, id: &str) -> bool {
        self.rule(rule).iter().any(|known| known == id)
    }

    /// Whether the record knows nothing at all.
    pub fn is_empty(&self) -> bool {
        self.rules.values().all(Vec::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_answers_which_rule_produced_an_id() {
        let mut record = Derivations::default();
        record
            .rules
            .insert("per-member".to_string(), vec!["ada".to_string()]);

        assert!(record.contains("per-member", "ada"));
        assert!(!record.contains("per-member", "zoe"));
        assert!(!record.contains("other", "ada"));
        assert!(!record.is_empty());
    }

    #[test]
    fn an_empty_record_is_empty_even_with_rule_keys() {
        let mut record = Derivations::default();
        record.rules.insert("per-member".to_string(), Vec::new());
        assert!(record.is_empty());
    }
}
