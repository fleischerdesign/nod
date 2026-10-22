//! Deletion policy for provider reconciliation (ADR-030).
//!
//! Two guards, both enforced by the core and never by the provider:
//! a protect list that forbids deleting named resources even when the fleet owns
//! them, and a per-run cap that refuses a plan which would delete too much at
//! once. A plan over the cap is refused **as a whole**: nothing half-applies.

use serde::{Deserialize, Serialize};

/// What the core allows a run to delete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Glob patterns on resource ids that must never be deleted.
    #[serde(default)]
    pub protect: Vec<String>,
    /// Maximum deletions per run; `0` forbids deletions entirely.
    #[serde(default)]
    pub max_deletions: usize,
}

impl Policy {
    /// Whether `id` matches any protect pattern.
    pub fn is_protected(&self, id: &str) -> bool {
        self.protect.iter().any(|pattern| glob_match(pattern, id))
    }

    /// Returns this policy narrowed by the operator's CLI flags.
    ///
    /// Narrowing is monotone: patterns are added, never removed, and the cap
    /// can only be lowered. A command line cannot disarm a declared guard —
    /// releasing a name or raising the cap is a declaration change, reviewed
    /// like any other.
    pub fn narrowed(&self, protect: &[String], max_deletions: Option<usize>) -> Policy {
        let mut narrow = self.clone();
        for pattern in protect {
            if !narrow.protect.contains(pattern) {
                narrow.protect.push(pattern.clone());
            }
        }
        if let Some(limit) = max_deletions {
            narrow.max_deletions = narrow.max_deletions.min(limit);
        }
        narrow
    }
}

/// Matches `value` against a pattern where `*` matches any run of characters.
///
/// Deliberately small: the protect list needs prefixes, suffixes and a whole
/// wildcard, not a regex engine in the core.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut rest = value;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            let Some(after) = rest.strip_prefix(part) else {
                return false;
            };
            rest = after;
        } else if index == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(position) = rest.find(part) else {
                return false;
            };
            rest = &rest[position + part.len()..];
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_pattern_matches_only_itself() {
        assert!(glob_match("TXT:example.com", "TXT:example.com"));
        assert!(!glob_match("TXT:example.com", "TXT:www.example.com"));
    }

    #[test]
    fn a_leading_wildcard_matches_a_suffix() {
        assert!(glob_match("*.example.com", "TXT:www.example.com"));
        assert!(glob_match("*.example.com", "A:www.example.com"));
        assert!(!glob_match("*.example.com", "TXT:example.org"));
    }

    #[test]
    fn a_middle_wildcard_matches_both_sides() {
        assert!(glob_match("TXT:*example.com", "TXT:nod-canary.example.com"));
        assert!(!glob_match("TXT:*example.com", "A:nod-canary.example.com"));
    }

    #[test]
    fn a_bare_wildcard_matches_everything() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn the_protect_list_matches_by_id() {
        let policy = Policy {
            protect: vec!["*:example.com".to_string(), "TXT:*.example.com".to_string()],
            max_deletions: 0,
        };
        assert!(policy.is_protected("A:example.com"));
        assert!(policy.is_protected("TXT:www.example.com"));
        assert!(!policy.is_protected("A:www.example.com"));
    }

    #[test]
    fn the_default_forbids_deletions() {
        assert_eq!(Policy::default().max_deletions, 0);
    }

    #[test]
    fn narrowing_adds_patterns_and_keeps_the_declared_ones() {
        let declared = Policy {
            protect: vec!["*:example.com".to_string()],
            max_deletions: 2,
        };
        let narrow = declared.narrowed(&["TXT:canary.example.com".to_string()], None);
        assert!(narrow.is_protected("A:example.com"));
        assert!(narrow.is_protected("TXT:canary.example.com"));
        assert_eq!(narrow.max_deletions, 2);
    }

    #[test]
    fn narrowing_lowers_the_cap_but_never_raises_it() {
        let declared = Policy {
            protect: Vec::new(),
            max_deletions: 1,
        };
        assert_eq!(declared.narrowed(&[], Some(0)).max_deletions, 0);
        assert_eq!(declared.narrowed(&[], Some(9)).max_deletions, 1);
    }

    #[test]
    fn narrowing_without_flags_changes_nothing() {
        let declared = Policy {
            protect: vec!["MX:*".to_string()],
            max_deletions: 3,
        };
        assert_eq!(declared.narrowed(&[], None), declared);
    }
}
