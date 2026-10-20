//! GitOps reconciliation state and reports (ADR-028).
//!
//! Two kinds of data live here, and the distinction matters:
//!
//! * [`SyncState`] is the reconciler's *memory* — what it last attempted and
//!   whether it is backing off. It is never consulted to decide whether a host
//!   is current; closures answer that question.
//! * [`SyncReport`] is the *outcome* of one cycle, expressed in domain terms so
//!   presentation never has to know how the deployment pipeline reports.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Version of the on-disk state document.
pub const STATE_VERSION: u32 = 1;

/// First backoff step after a failed cycle, in seconds.
const BACKOFF_BASE_SECS: u64 = 30;

/// Upper bound for the exponential backoff, in seconds.
const BACKOFF_MAX_SECS: u64 = 600;

/// Seconds since the Unix epoch, in UTC. Used for state and report timestamps.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The reconciler's memory across cycles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// Schema version of this document.
    pub version: u32,
    /// Repository identity, for operator inspection only.
    pub repo: String,
    /// Branch being reconciled.
    pub branch: String,
    /// Revision whose attempt last started.
    pub last_attempt_rev: Option<String>,
    /// Revision that was last successfully activated.
    pub last_applied_rev: Option<String>,
    /// Consecutive failed cycles; drives the backoff.
    pub consecutive_failures: u32,
    /// Unix time before which no new cycle starts.
    pub backoff_until: Option<u64>,
    /// Marker written before the first activation and cleared after a report.
    pub in_progress: Option<InProgress>,
    /// Hosts the last cycle could not reach. While this is non-empty, the next
    /// cycles probe only these hosts instead of re-evaluating the fleet.
    #[serde(default)]
    pub unreachable: Vec<String>,
}

/// A cycle that started activating and has not reported yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InProgress {
    /// Revision being deployed.
    pub rev: String,
    /// Unix time the cycle started.
    pub started_at: u64,
    /// Hosts the cycle intended to activate.
    pub hosts: Vec<String>,
}

impl SyncState {
    /// A fresh state document for one repository and branch.
    pub fn new(repo: impl Into<String>, branch: impl Into<String>) -> Self {
        Self {
            version: STATE_VERSION,
            repo: repo.into(),
            branch: branch.into(),
            last_attempt_rev: None,
            last_applied_rev: None,
            consecutive_failures: 0,
            backoff_until: None,
            in_progress: None,
            unreachable: Vec::new(),
        }
    }

    /// Seconds still to wait before the next cycle, zero when not backing off.
    pub fn backoff_remaining(&self, now: u64) -> u64 {
        self.backoff_until
            .map(|until| until.saturating_sub(now))
            .unwrap_or(0)
    }

    /// Records a failed cycle and returns the backoff that now applies.
    pub fn record_failure(&mut self, now: u64) -> u64 {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let shift = self.consecutive_failures.saturating_sub(1).min(6);
        let backoff = BACKOFF_BASE_SECS
            .saturating_mul(1u64 << shift)
            .min(BACKOFF_MAX_SECS);
        self.backoff_until = Some(now.saturating_add(backoff));
        self.in_progress = None;
        backoff
    }

    /// Records a successful cycle: the backoff resets and the revision is
    /// remembered as applied.
    pub fn record_success(&mut self, rev: impl Into<String>) {
        self.last_applied_rev = Some(rev.into());
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.in_progress = None;
        self.unreachable.clear();
    }

    /// Marks the start of an activation phase.
    pub fn begin_progress(&mut self, rev: impl Into<String>, hosts: Vec<String>, now: u64) {
        let rev = rev.into();
        self.last_attempt_rev = Some(rev.clone());
        self.in_progress = Some(InProgress {
            rev,
            started_at: now,
            hosts,
        });
    }
}

/// What happened to one host in a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncHostResult {
    /// Activated successfully.
    Deployed,
    /// Closure already matched; nothing was activated.
    Unchanged,
    /// Activation failed and the host returned to its previous generation.
    RolledBack,
    /// Activation failed and the host stayed failed.
    Failed,
    /// The host did not answer; retried next cycle.
    Unreachable,
    /// Selected but not attempted (dry run, or the cycle aborted).
    Skipped,
}

impl SyncHostResult {
    /// `true` when the host needs no further action.
    pub fn is_settled(self) -> bool {
        match self {
            SyncHostResult::Deployed | SyncHostResult::Unchanged => true,
            SyncHostResult::RolledBack
            | SyncHostResult::Failed
            | SyncHostResult::Unreachable
            | SyncHostResult::Skipped => false,
        }
    }

    /// `true` when the host failed in a way that needs operator attention.
    ///
    /// An unreachable host is not a failure: it is skipped and retried, and a
    /// cycle that only saw unreachable hosts must not back off or claim the
    /// fleet is current.
    pub fn is_failure(self) -> bool {
        matches!(self, SyncHostResult::Failed | SyncHostResult::RolledBack)
    }
}

/// One host's result within a cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncHostReport {
    /// Host name.
    pub host: String,
    /// Outcome.
    pub result: SyncHostResult,
    /// Failure detail, when the outcome is not settled.
    pub detail: Option<String>,
}

impl SyncHostReport {
    /// Builds a settled or failed host report.
    pub fn new(host: impl Into<String>, result: SyncHostResult) -> Self {
        Self {
            host: host.into(),
            result,
            detail: None,
        }
    }

    /// Attaches failure detail.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The outcome of one reconciliation cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// Revision the cycle ended at (the local head after integration).
    pub revision: String,
    /// Revision the cycle started at, when known.
    pub previous_revision: Option<String>,
    /// `true` only when every selected host is settled and none failed.
    pub applied: bool,
    /// Per-host outcomes.
    pub hosts: Vec<SyncHostReport>,
    /// Why the cycle refused to act, when it did.
    pub refused: Option<String>,
    /// Why the cycle failed, when it did.
    pub error: Option<String>,
    /// Seconds the reconciler will wait before the next attempt.
    pub backoff_secs: u64,
}

impl SyncReport {
    /// A cycle that did nothing because it was refused before acting.
    pub fn refused(revision: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
            previous_revision: None,
            applied: false,
            hosts: Vec::new(),
            refused: Some(reason.into()),
            error: None,
            backoff_secs: 0,
        }
    }

    /// A cycle that failed after it started acting.
    pub fn failed(revision: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
            previous_revision: None,
            applied: false,
            hosts: Vec::new(),
            refused: None,
            error: Some(error.into()),
            backoff_secs: 0,
        }
    }

    /// `true` when the cycle neither refused nor failed.
    pub fn is_success(&self) -> bool {
        self.refused.is_none() && self.error.is_none() && self.applied
    }

    /// Hosts that were actually activated.
    pub fn deployed(&self) -> Vec<&SyncHostReport> {
        self.hosts
            .iter()
            .filter(|h| h.result == SyncHostResult::Deployed)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_and_is_capped() {
        let mut state = SyncState::new("repo", "main");
        assert_eq!(state.record_failure(100), 30);
        assert_eq!(state.record_failure(200), 60);
        assert_eq!(state.record_failure(300), 120);
        for _ in 0..10 {
            state.record_failure(400);
        }
        assert_eq!(state.backoff_until, Some(400 + BACKOFF_MAX_SECS));
    }

    #[test]
    fn success_clears_backoff_and_records_revision() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(100);
        state.record_success("abc");
        assert_eq!(state.last_applied_rev.as_deref(), Some("abc"));
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.backoff_until, None);
        assert_eq!(state.backoff_remaining(100), 0);
    }

    #[test]
    fn backoff_remaining_never_underflows() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(100);
        assert_eq!(state.backoff_remaining(1000), 0);
    }

    #[test]
    fn progress_marker_records_attempt_and_hosts() {
        let mut state = SyncState::new("repo", "main");
        state.begin_progress("rev1", vec!["a".into()], 42);
        let progress = state.in_progress.as_ref().unwrap();
        assert_eq!(progress.rev, "rev1");
        assert_eq!(progress.started_at, 42);
        assert_eq!(state.last_attempt_rev.as_deref(), Some("rev1"));
    }

    #[test]
    fn refused_reports_are_not_success() {
        let report = SyncReport::refused("abc", "dirty worktree");
        assert!(!report.is_success());
        assert!(report.refused.is_some());
        assert!(report.error.is_none());
    }
}
