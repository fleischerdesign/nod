//! Read-only view of the reconciler (ADR-028).
//!
//! The command answers three questions without touching the fleet: where is the
//! checkout, where is the branch, and what did the last cycle record. Drift is
//! opt-in because it builds every selected host's closure; the cheap view is
//! the one an operator or an agent asks for most often.

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::sync_inspect::inspect_hosts;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::sync::{
    now_secs, HostCondition, HostStateEntry, ProbePolicy, RetryPolicy, SyncHostStatus, SyncState,
    SyncStatus,
};
use crate::domain::watch::SyncOptions;

/// Builds a [`SyncStatus`] snapshot.
pub struct SyncStatusUseCase {
    ctx: Arc<AppContext>,
}

impl SyncStatusUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Reports revisions and stored state; inspects hosts when `hosts` is given.
    pub async fn execute(
        &self,
        flake_path: &Path,
        options: &SyncOptions,
        hosts: Option<Vec<HostEntity>>,
    ) -> Result<SyncStatus, NodError> {
        let git = self.ctx.git_port()?;
        let state_store = self.ctx.sync_state()?;
        let fresh = || {
            SyncState::new(
                format!("{}/{}", options.remote, options.branch),
                options.branch.clone(),
            )
        };
        // An unreadable state is reported, not fatal: the revisions are still
        // useful, and a status command that dies on a file mode helps nobody.
        let (state, state_error) = match state_store.load().await {
            Ok(Some(state)) => (state, None),
            Ok(None) => (fresh(), None),
            Err(error) => (fresh(), Some(error.to_string())),
        };

        let local_revision = git.local_head(flake_path).await?;
        let remote_revision = git
            .remote_head(flake_path, &options.remote, &options.branch)
            .await
            .ok();

        // `None` means "cannot be answered", not "in sync": an unreachable
        // remote and a stale checkout are different states and stay distinct.
        let behind = match (&remote_revision, &state.last_applied_rev) {
            (Some(remote), Some(applied)) => Some(remote != applied),
            (Some(_), None) => Some(true),
            (None, _) => None,
        };

        let mut host_statuses = Vec::new();
        if let Some(hosts) = hosts {
            let inspections = inspect_hosts(
                &self.ctx,
                flake_path,
                hosts,
                options.concurrency,
                options.verbose,
                // A status query was asked to observe: it must not inherit the
                // daemon's retry memory and report a host it did not probe.
                &ProbePolicy::probe_all(),
            )
            .await?;
            host_statuses = inspections
                .into_iter()
                .map(|inspection| SyncHostStatus {
                    host: inspection.host.name.clone(),
                    drifted: inspection.drifted(),
                    unreachable: inspection.is_unreachable(),
                    current: inspection.current.map(|path| path.display().to_string()),
                    target: inspection.target.map(|path| path.display().to_string()),
                    health: state
                        .hosts
                        .get(&inspection.host.name)
                        .and_then(|memory| memory.health.clone()),
                })
                .collect();
        }

        let backoff_secs = state.backoff_remaining(now_secs());
        let policy = RetryPolicy::from_options(options);
        let conditions = state
            .hosts
            .iter()
            .map(|(host, memory)| HostStateEntry {
                host: host.clone(),
                label: if memory.condition.is_gone(now_secs(), policy.gone_after_secs) {
                    "gone".to_string()
                } else {
                    memory.condition.label().to_string()
                },
                since: memory.condition.since(),
                next_probe_in: match memory.condition {
                    HostCondition::Offline { next_probe_at, .. } => {
                        Some(next_probe_at.saturating_sub(now_secs()))
                    }
                    _ => None,
                },
                detail: match &memory.condition {
                    HostCondition::Misconfigured { detail, .. } => Some(detail.clone()),
                    HostCondition::Offline { attempts, .. } => Some(match attempts {
                        1 => "1 failed probe".to_string(),
                        failed => format!("{failed} failed probes"),
                    }),
                    HostCondition::Reachable => None,
                },
            })
            .collect();

        Ok(SyncStatus {
            local_revision,
            remote_revision,
            applied_revision: state.last_applied_rev,
            verified_revision: state.last_verified_rev,
            behind,
            backoff_secs,
            observed_at: state.saved_at,
            conditions,
            hosts: host_statuses,
            state_error,
        })
    }
}
