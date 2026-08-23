//! Domain audit value objects (ADR-003 observability).
//!
//! `AuditEntry` is the immutable, dependency-free record of one deployment
//! outcome. It crosses the `AuditStorePort` seam so the persistent adapter
//! and the audit-log use case agree on shape (ADR-001).

use serde::{Deserialize, Serialize};

/// One recorded deployment outcome in the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// The host that was deployed.
    pub host_name: String,
    /// The terminal outcome (`completed`, `rolled_back`, `failed`, ...).
    pub outcome: String,
    /// Unix time (whole seconds) the entry was recorded at.
    pub recorded_at: u64,
}

impl AuditEntry {
    /// Builds an entry for `host`/`outcome` at `recorded_at` unix seconds.
    pub fn new(host_name: impl Into<String>, outcome: impl Into<String>, recorded_at: u64) -> Self {
        Self {
            host_name: host_name.into(),
            outcome: outcome.into(),
            recorded_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_round_trips_through_json() {
        let entry = AuditEntry::new("atlas", "completed", 1_700_000_000);
        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
        assert_eq!(back.host_name, "atlas");
        assert_eq!(back.outcome, "completed");
        assert_eq!(back.recorded_at, 1_700_000_000);
    }
}
