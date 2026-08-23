//! `HealthCheckUseCase`: post-deployment systemd health verification across a
//! set of hosts (ADR-003 observability). Composes over `HealthCheckerPort`.
//!
//! This is the Application-layer policy that turns the raw single-host
//! `verify_health` bool into a per-host verdict list; the concrete probe
//! (systemd) lives behind the port so tests run against a mock.

use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// One host's post-deployment health verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostHealth {
    /// Host that was verified.
    pub host_name: String,
    /// True when the active system passed its health probes.
    pub healthy: bool,
}

impl HostHealth {
    /// Builds a verdict for `host_name`.
    pub fn new(host_name: impl Into<String>, healthy: bool) -> Self {
        Self {
            host_name: host_name.into(),
            healthy,
        }
    }
}

/// Executes post-deployment health verification for one or more hosts.
pub struct HealthCheckUseCase {
    ctx: Arc<AppContext>,
}

impl HealthCheckUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Verifies every `hosts` entry through the resolved health checker.
    ///
    /// A probe failure on one host aborts the whole verification with the
    /// typed error (the caller chooses whether to keep going).
    pub async fn execute(&self, hosts: Vec<HostEntity>) -> Result<Vec<HostHealth>, NodError> {
        let checker = self.ctx.health_checker()?;
        let mut verdicts = Vec::<HostHealth>::with_capacity(hosts.len());
        for host in hosts {
            let healthy = checker.verify_health(&host).await?;
            verdicts.push(HostHealth::new(&host.name, healthy));
        }
        Ok(verdicts)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::health_checker::HealthCheckerPort;
    use async_trait::async_trait;
    use mockall::mock;

    mock! {
        FakeHealthChecker {}
        #[async_trait]
        impl HealthCheckerPort for FakeHealthChecker {
            async fn verify_health(&self, host: &HostEntity) -> Result<bool, NodError>;
        }
    }

    fn ctx_with(checker: MockFakeHealthChecker) -> Arc<AppContext> {
        use crate::infrastructure::deployment::local_deployer::LocalDeployer;
        use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
        Arc::new(
            AppContext::new(
                Arc::new(crate::infrastructure::nix::cli_evaluator::NixCliEvaluator::new()),
                Arc::new(LocalDeployer::new()),
                Arc::new(SshCliDeployer::new()),
            )
            .with_health_checker(Arc::new(checker)),
        )
    }

    #[tokio::test]
    async fn healthy_host_produces_a_healthy_verdict() {
        let mut checker = MockFakeHealthChecker::new();
        checker
            .expect_verify_health()
            .times(1)
            .returning(|_| Ok(true));

        let ctx = ctx_with(checker);
        let use_case = HealthCheckUseCase::new(ctx);
        let host = HostEntity::new("jello", "jello-machine", true);

        let verdicts = use_case.execute(vec![host]).await.unwrap();
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0], HostHealth::new("jello", true));
    }

    #[tokio::test]
    async fn unhealthy_host_produces_a_failed_verdict() {
        let mut checker = MockFakeHealthChecker::new();
        checker
            .expect_verify_health()
            .times(1)
            .returning(|_| Ok(false));

        let ctx = ctx_with(checker);
        let use_case = HealthCheckUseCase::new(ctx);

        let verdicts = use_case
            .execute(vec![HostEntity::new("atlas", "10.0.0.8", false)])
            .await
            .unwrap();
        assert_eq!(verdicts, vec![HostHealth::new("atlas", false)]);
    }

    #[tokio::test]
    async fn one_verdict_per_host_in_order() {
        let mut checker = MockFakeHealthChecker::new();
        checker
            .expect_verify_health()
            .times(2)
            .returning(|host| Ok(host.name == "atlas"));

        let ctx = ctx_with(checker);
        let use_case = HealthCheckUseCase::new(ctx);

        let hosts = vec![
            HostEntity::new("jello", "jello-machine", true),
            HostEntity::new("atlas", "10.0.0.8", false),
        ];
        let verdicts = use_case.execute(hosts).await.unwrap();
        assert_eq!(verdicts[0].host_name, "jello");
        assert!(!verdicts[0].healthy);
        assert_eq!(verdicts[1].host_name, "atlas");
        assert!(verdicts[1].healthy);
    }

    #[tokio::test]
    async fn checker_failure_propagates_as_typed_error() {
        let mut checker = MockFakeHealthChecker::new();
        checker
            .expect_verify_health()
            .times(1)
            .returning(|_| Err(NodError::healthcheck("systemctl failed to launch")));

        let ctx = ctx_with(checker);
        let use_case = HealthCheckUseCase::new(ctx);

        let err = use_case
            .execute(vec![HostEntity::new("jello", "jello-machine", true)])
            .await
            .err()
            .unwrap();
        assert!(matches!(err, NodError::HealthCheck { .. }));
    }
}
