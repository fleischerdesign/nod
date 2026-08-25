//! Domain entities for file watching and GitOps reconciliation (ADR-022).

use serde::{Deserialize, Serialize};

/// Options controlling interactive watch mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchOptions {
    /// Polling interval in seconds when checking file modifications.
    pub poll_interval_secs: u64,
    /// Debounce duration in milliseconds before triggering evaluation.
    pub debounce_ms: u64,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            poll_interval_secs: 2,
            debounce_ms: 500,
        }
    }
}

/// Options controlling pull-based GitOps synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOptions {
    /// Upstream Git remote name (default: `origin`).
    pub remote: String,
    /// Target Git branch (default: `main`).
    pub branch: String,
    /// Reconciliation interval in seconds.
    pub interval_secs: u64,
    /// Preview git changes and build without activating.
    pub dry_run: bool,
    /// Run once and exit rather than looping.
    pub once: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            remote: "origin".to_string(),
            branch: "main".to_string(),
            interval_secs: 300,
            dry_run: false,
            once: false,
        }
    }
}

/// Outcome of a GitOps synchronization iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// Current head commit hash.
    pub commit_hash: String,
    /// Whether changes were detected and applied.
    pub applied: bool,
    /// Error message if reconciliation failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_reports_serialize_and_deserialize() {
        let report = SyncReport {
            commit_hash: "a1b2c3d".to_string(),
            applied: true,
            error: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let parsed: SyncReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, parsed);
    }
}
