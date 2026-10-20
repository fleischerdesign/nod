//! Domain entities for file watching and GitOps reconciliation (ADR-022).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Random spread applied to the interval, in percent (0-50).
    pub jitter_percent: u8,
    /// Target specifiers; empty means the caller must not guess (ADR-028).
    pub targets: Vec<String>,
    /// Select every discovered host.
    pub all: bool,
    /// Narrow the selection to hosts carrying this tag.
    pub tag: Option<String>,
    /// Narrow the selection to hosts with this role.
    pub role: Option<String>,
    /// Directory holding the reconciler state document.
    pub state_dir: PathBuf,
    /// Rollout strategy (`all` / `canary` / `batch`).
    pub strategy: String,
    /// Wave size for `batch`.
    pub batch_size: usize,
    /// Maximum in-flight hosts.
    pub concurrency: usize,
    /// Abort the run on the first failure.
    pub fail_fast: bool,
    /// Roll an unhealthy host back to its previous generation.
    pub auto_rollback: bool,
    /// Allow integrating a revision that is not a descendant of the applied one.
    pub allow_downgrade: bool,
    /// Require a verified signature on the integrated revision.
    pub require_signature: bool,
    /// Hosts that must be deployed after every other target.
    pub last_hosts: Vec<String>,
    /// Commands run after each cycle; they receive the report as JSON on stdin.
    pub notify: Vec<String>,
    /// Emit the report as JSON.
    pub json: bool,
    /// Emit per-step detail while building and deploying.
    pub verbose: bool,
    /// Preview git changes and build without activating.
    pub dry_run: bool,
    /// Preview the activation per host (implies `dry_run`).
    pub plan: bool,
    /// Run once and exit rather than looping.
    pub once: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            remote: "origin".to_string(),
            branch: "main".to_string(),
            interval_secs: 60,
            jitter_percent: 10,
            targets: Vec::new(),
            all: false,
            tag: None,
            role: None,
            state_dir: PathBuf::from("/var/lib/nod"),
            strategy: "batch".to_string(),
            batch_size: 1,
            concurrency: 4,
            fail_fast: false,
            auto_rollback: true,
            allow_downgrade: false,
            require_signature: false,
            last_hosts: Vec::new(),
            notify: Vec::new(),
            json: false,
            verbose: false,
            dry_run: false,
            plan: false,
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
