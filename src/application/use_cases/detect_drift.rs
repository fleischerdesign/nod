//! `DetectDriftUseCase`: compare each host's live (active) closure against the
//! closure the flake would build today (ADR-003 observability).
//!
//! The flake closure comes from `EvaluatorPort::build_toplevel`; the live
//! closure from `DeployerPort::current_closure` resolved for that host's
//! target. A host with no resolvable live closure is conservatively flagged as
//! drifted (the flake and the machine cannot be confirmed in sync).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// The drift report for one host after comparing its two closures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReport {
    /// Host the report belongs to.
    pub host_name: String,
    /// The live/active closure when it could be resolved.
    pub active_closure: Option<PathBuf>,
    /// The freshly built flake closure for the host.
    pub flake_closure: Option<PathBuf>,
    /// True when the closures differ (or the live closure is unknown).
    pub drifted: bool,
}

/// Compares every host's live closure against its flake closure.
pub struct DetectDriftUseCase {
    ctx: Arc<AppContext>,
}

impl DetectDriftUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Compares the live and flake closures for one `host`.
    pub async fn execute(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<DriftReport, NodError> {
        let evaluator = self.ctx.evaluator();
        let flake_closure = evaluator.build_toplevel(flake_path, &host.name, verbose).await?;

        let deployer = self.ctx.deployer_for(host);
        let active_closure = deployer.current_closure(host).await?;

        let drifted = match &active_closure {
            Some(active) => active != &flake_closure,
            None => true,
        };

        Ok(DriftReport {
            host_name: host.name.clone(),
            active_closure,
            flake_closure: Some(flake_closure),
            drifted,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &HostEntity) -> Result<(), NodError>;
        }
    }

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel(&self, flake_path: &Path, host_name: &str, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    /// Context with the evaluator and both deployer slots bound to mocks.
    fn ctx_with(
        eval: MockFakeEvaluator,
        local: MockFakeDeployer,
        ssh: MockFakeDeployer,
    ) -> Arc<AppContext> {
        Arc::new(AppContext::new(
            Arc::new(eval),
            Arc::new(local),
            Arc::new(ssh),
        ))
    }

    #[tokio::test]
    async fn differing_closures_report_drift() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-flake")));

        let mut local = MockFakeDeployer::new();
        local
            .expect_current_closure()
            .times(1)
            .returning(|_| Ok(Some(PathBuf::from("/nix/store/bbb-live"))));

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DetectDriftUseCase::new(ctx);

        let report = use_case.execute(&host, Path::new("/tmp/flake"), false).await.unwrap();
        assert!(report.drifted);
        assert_eq!(report.active_closure, Some(PathBuf::from("/nix/store/bbb-live")));
        assert_eq!(report.flake_closure, Some(PathBuf::from("/nix/store/aaa-flake")));
    }

    #[tokio::test]
    async fn matching_closures_report_in_sync() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-flake")));

        let mut local = MockFakeDeployer::new();
        local
            .expect_current_closure()
            .times(1)
            .returning(|_| Ok(Some(PathBuf::from("/nix/store/aaa-flake"))));

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DetectDriftUseCase::new(ctx);

        let report = use_case.execute(&host, Path::new("/tmp/flake"), false).await.unwrap();
        assert!(!report.drifted);
    }

    #[tokio::test]
    async fn unknown_active_closure_counts_as_drift() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-flake")));

        let mut local = MockFakeDeployer::new();
        local.expect_current_closure().times(1).returning(|_| Ok(None));

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DetectDriftUseCase::new(ctx);

        let report = use_case.execute(&host, Path::new("/tmp/flake"), false).await.unwrap();
        assert!(report.drifted);
        assert_eq!(report.active_closure, None);
    }

    #[tokio::test]
    async fn remote_host_resolution_goes_through_the_ssh_slot() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-flake")));

        let mut ssh = MockFakeDeployer::new();
        ssh.expect_current_closure()
            .times(1)
            .returning(|_| Ok(Some(PathBuf::from("/nix/store/bbb-live"))));

        let ctx = ctx_with(eval, MockFakeDeployer::new(), ssh);
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let use_case = DetectDriftUseCase::new(ctx);

        let report = use_case.execute(&host, Path::new("/tmp/flake"), false).await.unwrap();
        assert!(report.drifted);
    }

    #[tokio::test]
    async fn closure_query_failure_propagates_typed_error() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-flake")));

        let mut local = MockFakeDeployer::new();
        local
            .expect_current_closure()
            .times(1)
            .returning(|_| Err(NodError::deployment("query failed")));

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DetectDriftUseCase::new(ctx);

        let err = use_case.execute(&host, Path::new("/tmp/flake"), false).await.err().unwrap();
        assert!(matches!(err, NodError::Deployment { .. }));
    }
}
