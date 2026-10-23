//! Fleet health refresh between deployments (ADR-029).
//!
//! A deployment observes only the hosts it activates. The fleet-wide
//! `verified` marker is a statement about *every* selected host, so without a
//! refresh an unchanged host's verdict ages out and the marker can never be
//! earned again without another full rollout. This use case re-observes the
//! hosts whose stored verdict is missing or stale; it never activates, copies
//! a closure or touches the store.

use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::health::HealthVerdict;
use crate::domain::host::HostEntity;
use crate::domain::sync::{HostCondition, SyncHostReport, SyncHostResult, SyncState};

/// Refreshes stale health verdicts without deploying.
pub struct VerifyFleetUseCase {
    ctx: Arc<AppContext>,
}

impl VerifyFleetUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Observes every host whose verdict is missing or older than
    /// `HEALTH_REFRESH_SECS`, and records what it learned.
    ///
    /// Hosts the sweep already knows are away are left to the reachability
    /// probe: their retry schedule is a transport question, not a health one.
    /// The returned reports name only the hosts that are not healthy, so a
    /// steady fleet stays quiet.
    pub async fn observe_stale(
        &self,
        hosts: &[HostEntity],
        state: &mut SyncState,
        now: u64,
    ) -> Result<Vec<SyncHostReport>, NodError> {
        let mut reports = Vec::new();

        for host in hosts {
            let memory = state.host(&host.name);
            let stale = memory.is_none_or(|memory| memory.health_is_stale(now));
            if !stale {
                continue;
            }
            if matches!(
                memory.map(|memory| &memory.condition),
                Some(HostCondition::Offline { .. } | HostCondition::Misconfigured { .. })
            ) {
                continue;
            }

            let verdict = match self.observe(host).await {
                Ok(verdict) => {
                    // A probe that produced a verdict talked to the host; the
                    // verified marker requires that condition explicitly.
                    state.observe_reachable(&host.name, now);
                    verdict
                }
                Err(error) => HealthVerdict::Unverified {
                    reason: error.to_string(),
                },
            };

            state.record_health(&host.name, verdict.clone(), now);
            if let Some(report) = report_for(host, &verdict) {
                reports.push(report);
            }
        }

        Ok(reports)
    }

    async fn observe(&self, host: &HostEntity) -> Result<HealthVerdict, NodError> {
        let profile = self.ctx.resolved_profile(host).await?;
        self.ctx
            .deployer_for(host)
            .observe_health(host, &profile)
            .await
    }
}

/// A report for a verdict that needs operator attention; `None` when healthy.
fn report_for(host: &HostEntity, verdict: &HealthVerdict) -> Option<SyncHostReport> {
    let (result, detail) = match verdict {
        HealthVerdict::Healthy => return None,
        HealthVerdict::Degraded { units } => (
            SyncHostResult::Failed,
            format!("degraded: {}", units.join(", ")),
        ),
        HealthVerdict::DegradedPreExisting { units } => (
            SyncHostResult::Failed,
            format!("pre-existing degradation: {}", units.join(", ")),
        ),
        HealthVerdict::Unverified { reason } => {
            (SyncHostResult::Unchanged, format!("unverified: {reason}"))
        }
        HealthVerdict::PendingReboot => (SyncHostResult::Unchanged, "pending reboot".to_string()),
    };

    Some(
        SyncHostReport::new(host.name.clone(), result)
            .with_detail(detail)
            .with_health(Some(verdict.clone())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use crate::domain::health::DeploymentReport;
    use crate::domain::host::{BuilderHost, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::sync::RetryPolicy;

    /// The use case never evaluates; this keeps the context honest.
    struct NoEvaluator;

    #[async_trait]
    impl EvaluatorPort for NoEvaluator {
        async fn discover_hosts(&self, _: &Path, _: bool) -> Result<Vec<HostEntity>, NodError> {
            unimplemented!("not used by this use case")
        }

        async fn build_toplevel<'a>(
            &self,
            _: &Path,
            _: &str,
            _: Option<String>,
            _: Option<&'a BuilderHost>,
            _: bool,
        ) -> Result<PathBuf, NodError> {
            unimplemented!("not used by this use case")
        }
    }

    /// Answers observations from a script and remembers who was asked.
    struct RecordingDeployer {
        verdicts: Mutex<Vec<Result<HealthVerdict, String>>>,
        observed: Mutex<Vec<String>>,
    }

    impl RecordingDeployer {
        fn new(verdicts: Vec<Result<HealthVerdict, String>>) -> Self {
            Self {
                verdicts: Mutex::new(verdicts.into_iter().rev().collect()),
                observed: Mutex::new(Vec::new()),
            }
        }

        fn observed(&self) -> Vec<String> {
            self.observed.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DeployerPort for RecordingDeployer {
        async fn check_reachability(&self, _: &HostEntity) -> Result<bool, NodError> {
            unimplemented!("not used by this use case")
        }

        async fn current_closure(
            &self,
            _: &HostEntity,
            _: &SshProfile,
        ) -> Result<Option<PathBuf>, NodError> {
            unimplemented!("not used by this use case")
        }

        async fn deploy_and_activate(
            &self,
            _: &HostEntity,
            _: &SshProfile,
            _: &Path,
            _: &str,
            _: bool,
        ) -> Result<DeploymentReport, NodError> {
            unimplemented!("not used by this use case")
        }

        async fn rollback(&self, _: &HostEntity, _: &SshProfile) -> Result<(), NodError> {
            unimplemented!("not used by this use case")
        }

        async fn observe_health(
            &self,
            host: &HostEntity,
            _: &SshProfile,
        ) -> Result<HealthVerdict, NodError> {
            self.observed.lock().unwrap().push(host.name.clone());
            match self.verdicts.lock().unwrap().pop() {
                Some(Ok(verdict)) => Ok(verdict),
                Some(Err(reason)) => Err(NodError::health_check(reason)),
                None => Err(NodError::internal("no verdict scripted")),
            }
        }
    }

    const NOW: u64 = 10_000;

    fn host(name: &str) -> HostEntity {
        HostEntity::new(name, "10.0.0.1", false)
    }

    fn context(deployer: Arc<RecordingDeployer>) -> Arc<AppContext> {
        Arc::new(AppContext::new(
            Arc::new(NoEvaluator),
            deployer.clone(),
            deployer,
        ))
    }

    fn state_with(entries: &[(&str, HealthVerdict, u64)]) -> SyncState {
        let mut state = SyncState::new("repo", "main");
        state.last_applied_rev = Some("abc".to_string());
        for (name, verdict, at) in entries {
            state.observe_reachable(name, *at);
            state.record_health(name, verdict.clone(), *at);
        }
        state
    }

    #[tokio::test]
    async fn only_missing_or_stale_verdicts_are_observed() {
        let deployer = Arc::new(RecordingDeployer::new(vec![Ok(HealthVerdict::Healthy)]));
        let hosts = vec![host("stale"), host("fresh")];
        let mut state = state_with(&[
            ("stale", HealthVerdict::Healthy, 1),
            ("fresh", HealthVerdict::Healthy, NOW),
        ]);

        let reports = VerifyFleetUseCase::new(context(deployer.clone()))
            .observe_stale(&hosts, &mut state, NOW)
            .await
            .expect("observation should run");

        assert!(reports.is_empty(), "a healthy refresh is silent");
        assert_eq!(deployer.observed(), vec!["stale"]);
        assert_eq!(state.host("stale").unwrap().health_at, Some(NOW));
    }

    #[tokio::test]
    async fn a_host_known_to_be_away_is_not_observed() {
        let deployer = Arc::new(RecordingDeployer::new(vec![]));
        let hosts = vec![host("away")];
        let mut state = SyncState::new("repo", "main");
        state.observe_offline(
            "away",
            NOW,
            &RetryPolicy {
                base_secs: 60,
                max_secs: 1800,
                gone_after_secs: 86_400,
            },
        );

        let reports = VerifyFleetUseCase::new(context(deployer.clone()))
            .observe_stale(&hosts, &mut state, NOW)
            .await
            .expect("observation should run");

        assert!(reports.is_empty());
        assert!(
            deployer.observed().is_empty(),
            "the retry schedule owns an away host"
        );
    }

    #[tokio::test]
    async fn a_failed_probe_is_recorded_as_unverified() {
        let deployer = Arc::new(RecordingDeployer::new(vec![Err(
            "connection refused".to_string()
        )]));
        let hosts = vec![host("web")];
        let mut state = SyncState::new("repo", "main");

        let reports = VerifyFleetUseCase::new(context(deployer))
            .observe_stale(&hosts, &mut state, NOW)
            .await
            .expect("observation should run");

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].result, SyncHostResult::Unchanged);
        assert!(reports[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("unverified"));
        assert!(matches!(
            state.host("web").unwrap().health,
            Some(HealthVerdict::Unverified { .. })
        ));
    }

    #[tokio::test]
    async fn a_degraded_verdict_becomes_a_failed_report() {
        let deployer = Arc::new(RecordingDeployer::new(vec![Ok(HealthVerdict::Degraded {
            units: vec!["web.service".to_string()],
        })]));
        let hosts = vec![host("web")];
        let mut state = SyncState::new("repo", "main");

        let reports = VerifyFleetUseCase::new(context(deployer))
            .observe_stale(&hosts, &mut state, NOW)
            .await
            .expect("observation should run");

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].result, SyncHostResult::Failed);
        assert!(reports[0]
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("web.service"));
    }
}
