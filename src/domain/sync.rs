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
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::watch::SyncOptions;

/// Version of the on-disk state document.
///
/// Version 2 replaced the flat `unreachable` list with per-host memory. The
/// state is memory, never the source of truth: an older document is discarded
/// and costs one sweep, not correctness.
pub const STATE_VERSION: u32 = 2;

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
    /// Revision where every host was activated **and** verified healthy.
    ///
    /// Distinct from `last_applied_rev` on purpose: reaching every host that
    /// answered is a deployment, not a proof. A host that was offline or could
    /// not be observed keeps this behind while `last_applied_rev` advances.
    #[serde(default)]
    pub last_verified_rev: Option<String>,
    /// Consecutive failed cycles; drives the backoff.
    pub consecutive_failures: u32,
    /// Unix time before which no new cycle starts.
    pub backoff_until: Option<u64>,
    /// Revision whose failure the current backoff belongs to. A backoff is a
    /// statement about one revision, not about the fleet: a newer revision
    /// clears it, because the failure cannot apply to code that did not exist.
    #[serde(default)]
    pub backoff_rev: Option<String>,
    /// Marker written before the first activation and cleared after a report.
    pub in_progress: Option<InProgress>,
    /// Unix time the document was last written. `None` for documents written
    /// before the field existed, which is why it is not part of the version.
    #[serde(default)]
    pub saved_at: Option<u64>,
    /// What the last cycles learned about each host, keyed by host name.
    #[serde(default)]
    pub hosts: BTreeMap<String, HostMemory>,
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

/// How a host that is not answering is retried (ADR-028).
///
/// The cadence is data, not a constant in the loop: a fleet with a laptop that
/// sleeps for days stretches it without touching code, and a fleet of servers
/// keeps it short without a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Delay before the first retry, in seconds.
    pub base_secs: u64,
    /// Ceiling for the retry delay, in seconds.
    pub max_secs: u64,
    /// Seconds offline after which a host is reported as gone (0 disables).
    pub gone_after_secs: u64,
}

impl RetryPolicy {
    /// Derives the policy from the reconciler options.
    pub fn from_options(options: &SyncOptions) -> Self {
        Self {
            base_secs: options.offline_backoff_secs.max(1),
            max_secs: options.offline_backoff_max_secs.max(1),
            gone_after_secs: options.gone_after_secs,
        }
    }

    /// Delay before the next probe after `attempts` consecutive failed probes.
    pub fn delay(&self, attempts: u32) -> u64 {
        let shift = attempts.saturating_sub(1).min(16);
        self.base_secs
            .saturating_mul(1u64 << shift)
            .min(self.max_secs.max(self.base_secs))
    }
}

/// What the reconciler knows about one host between cycles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostCondition {
    /// The host answered when it was last probed.
    Reachable,
    /// The host did not answer: off, rebooting, or cut off.
    Offline {
        /// Unix time the outage was first observed.
        since: u64,
        /// Consecutive failed probes.
        attempts: u32,
        /// Unix time before which the host is not probed again.
        next_probe_at: u64,
    },
    /// The host answers but refuses us: a wrong key or a changed host key.
    ///
    /// This is a configuration defect, not an outage. Skipping it like an
    /// offline host would hide a broken rollout behind a plausible label.
    Misconfigured {
        /// Unix time the condition was first observed.
        since: u64,
        /// Operator-facing reason.
        detail: String,
    },
}

impl HostCondition {
    /// Short label for status output and audit records.
    pub fn label(&self) -> &'static str {
        match self {
            HostCondition::Reachable => "reachable",
            HostCondition::Offline { .. } => "offline",
            HostCondition::Misconfigured { .. } => "misconfigured",
        }
    }

    /// Unix time the condition was first observed, when it is a defect.
    pub fn since(&self) -> Option<u64> {
        match self {
            HostCondition::Reachable => None,
            HostCondition::Offline { since, .. } | HostCondition::Misconfigured { since, .. } => {
                Some(*since)
            }
        }
    }

    /// `true` when the host has been away long enough to be treated as gone
    /// rather than temporarily unreachable.
    pub fn is_gone(&self, now: u64, threshold_secs: u64) -> bool {
        if threshold_secs == 0 {
            return false;
        }
        match self {
            HostCondition::Offline { since, .. } => now.saturating_sub(*since) >= threshold_secs,
            _ => false,
        }
    }
}

/// One host's remembered condition and when it was last seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMemory {
    /// What the last probe learned.
    pub condition: HostCondition,
    /// Unix time the host was last observed answering, zero when never.
    pub last_seen: u64,
    /// What the last deployment concluded about this host's health.
    ///
    /// Persisted so `nod status` can name a degraded host without re-probing,
    /// and so an unreachable host cannot silently keep a stale "healthy".
    #[serde(default)]
    pub health: Option<crate::domain::health::HealthVerdict>,
}

/// The hosts a sweep may probe now, and why the rest wait.
///
/// Kept as a value object so the decision is pure and testable, and so a
/// caller that genuinely wants a full sweep (`nod sync status --drift`) says
/// so explicitly instead of inheriting the daemon's memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbePolicy {
    deferred: BTreeMap<String, String>,
}

impl ProbePolicy {
    /// Probes every host: the caller asked for observation, not for memory.
    pub fn probe_all() -> Self {
        Self::default()
    }

    /// Defers `host` with a reason.
    pub fn defer(&mut self, host: impl Into<String>, reason: impl Into<String>) {
        self.deferred.insert(host.into(), reason.into());
    }

    /// The reason `host` is not probed, when it is deferred.
    pub fn reason(&self, host: &str) -> Option<&str> {
        self.deferred.get(host).map(String::as_str)
    }

    /// `true` when every host may be probed.
    pub fn is_empty(&self) -> bool {
        self.deferred.is_empty()
    }
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
            last_verified_rev: None,
            consecutive_failures: 0,
            backoff_until: None,
            backoff_rev: None,
            in_progress: None,
            saved_at: None,
            hosts: BTreeMap::new(),
        }
    }

    /// What is known about `host`, when anything is.
    pub fn host(&self, host: &str) -> Option<&HostMemory> {
        self.hosts.get(host)
    }

    /// The probe policy implied by what the last cycles learned.
    pub fn probe_policy(&self, now: u64, policy: &RetryPolicy) -> ProbePolicy {
        let mut deferred = BTreeMap::new();
        for (name, memory) in &self.hosts {
            if let HostCondition::Offline {
                since,
                attempts,
                next_probe_at,
            } = memory.condition
            {
                if next_probe_at > now {
                    let wait = (next_probe_at - now).min(policy.max_secs.max(1));
                    deferred.insert(
                        name.clone(),
                        format!(
                            "offline since {since} ({attempts} probe(s) failed), next probe in {wait}s"
                        ),
                    );
                }
            }
        }
        ProbePolicy { deferred }
    }

    /// Records that `host` answered.
    pub fn observe_reachable(&mut self, host: &str, now: u64) {
        self.hosts.insert(
            host.to_string(),
            HostMemory {
                condition: HostCondition::Reachable,
                last_seen: now,
                health: self
                    .hosts
                    .get(host)
                    .and_then(|memory| memory.health.clone()),
            },
        );
    }

    /// Records a failed probe and returns the delay before the next one.
    ///
    /// The `since` of an ongoing outage is preserved, so the report says how
    /// long a host has been away rather than when it was last tried.
    pub fn observe_offline(&mut self, host: &str, now: u64, policy: &RetryPolicy) -> u64 {
        let (since, attempts) = match self.hosts.get(host).map(|m| &m.condition) {
            Some(HostCondition::Offline {
                since, attempts, ..
            }) => (*since, attempts.saturating_add(1)),
            _ => (now, 1),
        };
        let delay = policy.delay(attempts);
        self.hosts.insert(
            host.to_string(),
            HostMemory {
                condition: HostCondition::Offline {
                    since,
                    attempts,
                    next_probe_at: now.saturating_add(delay),
                },
                last_seen: self
                    .hosts
                    .get(host)
                    .map(|memory| memory.last_seen)
                    .unwrap_or(0),
                // An unreachable host has no current health, whatever it had
                // before: its last verdict is not evidence about this cycle.
                health: None,
            },
        );
        delay
    }

    /// Records that `host` answers but refuses us: a wrong key or a changed
    /// host key. This is not an outage and must reach the operator.
    pub fn observe_misconfigured(&mut self, host: &str, now: u64, detail: impl Into<String>) {
        let since = match self.hosts.get(host).map(|m| &m.condition) {
            Some(HostCondition::Misconfigured { since, .. }) => *since,
            _ => now,
        };
        self.hosts.insert(
            host.to_string(),
            HostMemory {
                condition: HostCondition::Misconfigured {
                    since,
                    detail: detail.into(),
                },
                last_seen: now,
                health: None,
            },
        );
    }

    /// Records the health verdict a deployment reached for `host`.
    ///
    /// A host the sweep has not seen yet is inserted as reachable: the
    /// deployment just talked to it, so that is the condition the memory
    /// should carry.
    pub fn record_health(
        &mut self,
        host: &str,
        verdict: crate::domain::health::HealthVerdict,
        now: u64,
    ) {
        let memory = self.hosts.entry(host.to_string()).or_insert(HostMemory {
            condition: HostCondition::Reachable,
            last_seen: now,
            health: None,
        });
        memory.health = Some(verdict);
        memory.last_seen = now;
    }

    /// Seconds still to wait before the next cycle, zero when not backing off.
    ///
    /// Capped at the backoff ceiling: a wall clock that jumped backwards must
    /// not turn a 10-minute pause into an hour-long one.
    pub fn backoff_remaining(&self, now: u64) -> u64 {
        self.backoff_until
            .map(|until| until.saturating_sub(now))
            .unwrap_or(0)
            .min(BACKOFF_MAX_SECS)
    }

    /// Records a failed cycle for `rev` and returns the backoff that applies.
    pub fn record_failure(&mut self, now: u64, rev: impl Into<String>) -> u64 {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let shift = self.consecutive_failures.saturating_sub(1).min(6);
        let backoff = BACKOFF_BASE_SECS
            .saturating_mul(1u64 << shift)
            .min(BACKOFF_MAX_SECS);
        self.backoff_until = Some(now.saturating_add(backoff));
        self.backoff_rev = Some(rev.into());
        self.in_progress = None;
        backoff
    }

    /// Drops a backoff that belonged to a superseded revision.
    ///
    /// Returns `true` when a backoff was cleared. The caller observed a remote
    /// head that is not the revision that failed, so waiting out the backoff
    /// would only delay the fix.
    pub fn supersede_backoff(&mut self, remote: &str) -> bool {
        if self.backoff_remaining(now_secs()) == 0 {
            return false;
        }
        if self.backoff_rev.as_deref() == Some(remote) {
            return false;
        }
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.backoff_rev = None;
        true
    }

    /// Records a successful cycle: the backoff resets and the revision is
    /// remembered as applied and verified.
    pub fn record_success(&mut self, rev: impl Into<String>) {
        let rev = rev.into();
        self.last_applied_rev = Some(rev.clone());
        self.last_verified_rev = Some(rev);
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.backoff_rev = None;
        self.in_progress = None;
    }

    /// Records a cycle that reached every host that answered, without claiming
    /// verification: a host was unreachable, unobserved or owed a reboot.
    pub fn record_deployed(&mut self, rev: impl Into<String>) {
        self.last_applied_rev = Some(rev.into());
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.backoff_rev = None;
        self.in_progress = None;
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
    /// What health verification concluded, when it ran (ADR-029).
    #[serde(default)]
    pub health: Option<crate::domain::health::HealthVerdict>,
}

impl SyncHostReport {
    /// Builds a settled or failed host report.
    pub fn new(host: impl Into<String>, result: SyncHostResult) -> Self {
        Self {
            host: host.into(),
            result,
            detail: None,
            health: None,
        }
    }

    /// Attaches the health verdict the deployment reached.
    pub fn with_health(mut self, health: Option<crate::domain::health::HealthVerdict>) -> Self {
        self.health = health;
        self
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
    /// `true` only when every selected host is settled **and** verified
    /// healthy. A deployment that merely ran is not a verification.
    #[serde(default)]
    pub verified: bool,
    /// Per-host outcomes.
    pub hosts: Vec<SyncHostReport>,
    /// Why the cycle refused to act, when it did.
    pub refused: Option<String>,
    /// Why the cycle failed, when it did.
    pub error: Option<String>,
    /// Seconds the reconciler will wait before the next attempt.
    pub backoff_secs: u64,
    /// What a plan would do per host; only populated for dry-run and plan mode.
    #[serde(default)]
    pub plan: Vec<SyncPlanEntry>,
}

/// One host's entry in a deployment plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPlanEntry {
    /// Host name.
    pub host: String,
    /// Closure the host runs now, when known.
    pub current: Option<PathBuf>,
    /// Closure the host would run.
    pub target: PathBuf,
    /// Units the activation would start, restart or stop, when the closure is
    /// available locally (`switch-to-configuration dry-activate`).
    #[serde(default)]
    pub units: Option<String>,
}

/// One host's line in a status report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncHostStatus {
    /// Host name.
    pub host: String,
    /// Whether the built closure differs from the live one.
    pub drifted: bool,
    /// Whether the host did not answer.
    pub unreachable: bool,
    /// Live closure, when known.
    pub current: Option<String>,
    /// Declared closure, when the host answered.
    pub target: Option<String>,
    /// What the last deployment concluded about this host's health.
    #[serde(default)]
    pub health: Option<crate::domain::health::HealthVerdict>,
}

/// One host's remembered condition, for status output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStateEntry {
    /// Host name.
    pub host: String,
    /// Condition label: `reachable`, `offline`, `misconfigured` or `gone`.
    pub label: String,
    /// Unix time the condition was first observed, when it is a defect.
    pub since: Option<u64>,
    /// Seconds until the next probe, when the host is deferred.
    pub next_probe_in: Option<u64>,
    /// Operator-facing detail, when the condition carries one.
    #[serde(default)]
    pub detail: Option<String>,
}

/// Snapshot of the reconciler's view: revisions, state and optional drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    /// Commit checked out in the reconciler's repository.
    pub local_revision: String,
    /// Commit the remote branch points at, when reachable.
    pub remote_revision: Option<String>,
    /// Revision the last settled cycle recorded.
    pub applied_revision: Option<String>,
    /// Revision where every host was activated **and** verified healthy.
    /// `None` says no cycle has proven the fleet healthy at a revision yet.
    #[serde(default)]
    pub verified_revision: Option<String>,
    /// Whether the applied revision differs from the remote one; `None` when
    /// the remote could not be read.
    pub behind: Option<bool>,
    /// Seconds until the next attempt, when backing off.
    pub backoff_secs: u64,
    /// When the reconciler last wrote this state; `None` when it cannot say.
    /// A verdict without an age is a claim without a date, and a claim without
    /// a date is what made "offline" ambiguous.
    #[serde(default)]
    pub observed_at: Option<u64>,
    /// What the state remembers about each host.
    #[serde(default)]
    pub conditions: Vec<HostStateEntry>,
    /// Per-host drift, populated on request.
    #[serde(default)]
    pub hosts: Vec<SyncHostStatus>,
    /// Why the state document could not be read, when it could not.
    #[serde(default)]
    pub state_error: Option<String>,
}

impl SyncReport {
    /// A cycle that did nothing because it was refused before acting.
    pub fn refused(revision: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
            previous_revision: None,
            applied: false,
            verified: false,
            hosts: Vec::new(),
            refused: Some(reason.into()),
            error: None,
            backoff_secs: 0,
            plan: Vec::new(),
        }
    }

    /// A cycle that failed after it started acting.
    pub fn failed(revision: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
            previous_revision: None,
            applied: false,
            verified: false,
            hosts: Vec::new(),
            refused: None,
            error: Some(error.into()),
            backoff_secs: 0,
            plan: Vec::new(),
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
        assert_eq!(state.record_failure(100, "a"), 30);
        assert_eq!(state.record_failure(200, "a"), 60);
        assert_eq!(state.record_failure(300, "a"), 120);
        for _ in 0..10 {
            state.record_failure(400, "a");
        }
        assert_eq!(state.backoff_until, Some(400 + BACKOFF_MAX_SECS));
    }

    #[test]
    fn success_clears_backoff_and_records_revision() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(100, "a");
        state.record_success("abc");
        assert_eq!(state.last_applied_rev.as_deref(), Some("abc"));
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.backoff_until, None);
        assert_eq!(state.backoff_remaining(100), 0);
    }

    #[test]
    fn a_deployment_without_verification_is_not_a_verified_revision() {
        let mut state = SyncState::new("repo", "main");
        state.record_deployed("abc");
        assert_eq!(state.last_applied_rev.as_deref(), Some("abc"));
        assert_eq!(
            state.last_verified_rev, None,
            "reaching every host that answered is not a proof"
        );

        state.record_success("abc");
        assert_eq!(state.last_verified_rev.as_deref(), Some("abc"));
    }

    #[test]
    fn an_offline_host_loses_its_health_verdict() {
        use crate::domain::health::HealthVerdict;

        let mut state = SyncState::new("repo", "main");
        let policy = RetryPolicy {
            base_secs: 60,
            max_secs: 1800,
            gone_after_secs: 86_400,
        };
        state.observe_reachable("atlas", 100);
        state.record_health("atlas", HealthVerdict::Healthy, 100);
        assert!(state.host("atlas").unwrap().health.is_some());

        state.observe_offline("atlas", 200, &policy);
        assert!(
            state.host("atlas").unwrap().health.is_none(),
            "a stale verdict is not evidence about this cycle"
        );
    }

    #[test]
    fn a_state_document_written_before_verified_revisions_still_loads() {
        let json = r#"{"version":2,"repo":"repo","branch":"main","last_attempt_rev":null,"last_applied_rev":"abc","consecutive_failures":0,"backoff_until":null,"in_progress":null,"hosts":{}}"#;
        let state: SyncState = serde_json::from_str(json).unwrap();
        assert_eq!(state.last_applied_rev.as_deref(), Some("abc"));
        assert_eq!(state.last_verified_rev, None);
    }

    #[test]
    fn backoff_remaining_never_underflows() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(100, "a");
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

    fn retry_policy() -> RetryPolicy {
        RetryPolicy {
            base_secs: 60,
            max_secs: 1800,
            gone_after_secs: 86_400,
        }
    }

    #[test]
    fn the_retry_delay_grows_and_is_capped() {
        let policy = retry_policy();
        assert_eq!(policy.delay(1), 60);
        assert_eq!(policy.delay(2), 120);
        assert_eq!(policy.delay(3), 240);
        assert_eq!(policy.delay(50), 1800);
    }

    #[test]
    fn a_host_is_deferred_until_its_retry_is_due() {
        let policy = retry_policy();
        let mut state = SyncState::new("repo", "main");
        state.observe_offline("atlas", 1_000, &policy);

        let early = state.probe_policy(1_010, &policy);
        assert!(
            early.reason("atlas").is_some(),
            "probing before the retry is due is the cost this exists to avoid"
        );

        let due = state.probe_policy(1_060, &policy);
        assert!(due.reason("atlas").is_none());
    }

    #[test]
    fn an_outage_keeps_its_original_start_time() {
        let policy = retry_policy();
        let mut state = SyncState::new("repo", "main");
        state.observe_offline("atlas", 1_000, &policy);
        state.observe_offline("atlas", 5_000, &policy);
        assert_eq!(
            state.host("atlas").and_then(|m| m.condition.since()),
            Some(1_000),
            "the report must say when the host went away, not when it was retried"
        );
    }

    #[test]
    fn a_refusal_is_stored_as_a_defect_not_an_outage() {
        let mut state = SyncState::new("repo", "main");
        state.observe_misconfigured("atlas", 1_000, "Permission denied (publickey)");
        let memory = state.host("atlas").unwrap();
        assert_eq!(memory.condition.label(), "misconfigured");
        assert!(!memory.condition.is_gone(200_000, 86_400));
    }

    #[test]
    fn a_host_offline_past_the_threshold_is_gone() {
        let policy = retry_policy();
        let mut state = SyncState::new("repo", "main");
        state.observe_offline("atlas", 1_000, &policy);
        let condition = &state.host("atlas").unwrap().condition;
        assert!(!condition.is_gone(10_000, policy.gone_after_secs));
        assert!(condition.is_gone(90_000, policy.gone_after_secs));
    }

    #[test]
    fn a_newer_revision_clears_the_backoff() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(now_secs(), "bad");
        assert!(
            state.supersede_backoff("fix"),
            "waiting out a backoff for code that does not exist is pure delay"
        );
        assert_eq!(state.backoff_remaining(now_secs()), 0);
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn the_same_revision_keeps_its_backoff() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(now_secs(), "bad");
        assert!(!state.supersede_backoff("bad"));
        assert!(state.backoff_remaining(now_secs()) > 0);
    }

    #[test]
    fn a_clock_that_jumped_back_cannot_stretch_the_pause() {
        let mut state = SyncState::new("repo", "main");
        state.record_failure(100_000, "a");
        // The wall clock now claims an hour earlier than the failure.
        assert!(state.backoff_remaining(96_400) <= BACKOFF_MAX_SECS);
    }
}
