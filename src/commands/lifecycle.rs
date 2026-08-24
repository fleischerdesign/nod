//! Shared execution pipeline for fleet deployment lifecycle commands
//! (`switch`, `test`, `boot`).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::pipeline::state_machine::DeploymentState;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::deploy_fleet::DeployFleetUseCase;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};

/// Input parameters for a lifecycle deployment command.
#[derive(Debug, Clone)]
pub struct LifecycleParams<'a> {
    pub target: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub role: Option<&'a str>,
    pub all: bool,
    pub dry_run: bool,
    pub concurrency: usize,
    pub strategy: &'a str,
    pub batch_size: usize,
    pub fail_fast: bool,
    pub auto_rollback: bool,
    pub verbose: bool,
    pub quiet: bool,
}

/// Executes a standard deployment lifecycle use case across resolved targets (ADR-010, ADR-013).
pub async fn execute_lifecycle(
    ctx: AppContext,
    flake_path: &Path,
    action: DeploymentAction,
    params: LifecycleParams<'_>,
) -> Result<(), NodError> {
    if params.concurrency == 0 {
        return Err(NodError::config("--concurrency must be at least 1"));
    }

    let parsed_strategy = RolloutStrategy::parse(params.strategy).ok_or_else(|| {
        NodError::config(format!("unknown rollout strategy '{}'", params.strategy))
    })?;

    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    let options = DeploymentOptions {
        dry_run: params.dry_run,
        concurrency: params.concurrency,
        strategy: parsed_strategy,
        batch_size: params.batch_size,
        fail_fast: params.fail_fast,
        auto_rollback: params.auto_rollback,
        action,
        verbose: params.verbose,
        out_link: None,
        builder: None,
    };

    let hosts = evaluator
        .discover_hosts_degraded(flake_path, params.verbose)
        .await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let targets = resolve_targets(
        hosts,
        &local_hostname,
        params.target,
        params.tag,
        params.role,
        params.all,
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            params.target.unwrap_or("local"),
            params.tag,
            params.role,
        ));
    }

    let mut staged = Vec::<HostEntity>::with_capacity(targets.len());
    for host in targets {
        staged.push(store.apply_to(host).await?);
    }

    let audit_store_res = ctx.audit_store();
    let use_case = DeployFleetUseCase::new(Arc::new(ctx));
    let summary = use_case.execute(staged, options, flake_path).await?;

    if !params.dry_run {
        if let Ok(audit_store) = audit_store_res {
            for outcome in &summary.outcomes {
                let outcome_str = match outcome.state {
                    DeploymentState::Completed => "completed",
                    DeploymentState::RolledBack => "rolled_back",
                    _ => "failed",
                };
                if let Err(e) = audit_store.record(&outcome.host_name, outcome_str).await {
                    tracing::warn!(
                        "Failed to record audit entry for {}: {}",
                        outcome.host_name,
                        e
                    );
                }
            }
        }
    }

    if !params.quiet {
        crate::commands::render_summary(&summary);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::{FleetDefaults, HostOverrides};
    use crate::domain::host::SshProfile;
    use crate::domain::ports::config_store::ConfigStorePort;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
        }
    }

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    mock! {
        FakeConfigStore {}
        #[async_trait]
        impl ConfigStorePort for FakeConfigStore {
            async fn resolve(&self, host: &HostEntity) -> Result<SshProfile, NodError>;
            async fn host_overrides(&self, name: &str) -> Result<HostOverrides, NodError>;
            async fn fleet_defaults(&self) -> Result<FleetDefaults, NodError>;
            async fn apply_to(&self, host: HostEntity) -> Result<HostEntity, NodError>;
        }
    }

    mock! {
        FakeAuditStore {}
        #[async_trait]
        impl crate::domain::ports::audit_store::AuditStorePort for FakeAuditStore {
            async fn record(&self, host_name: &str, outcome: &str) -> Result<(), NodError>;
            async fn entries(&self, host: Option<String>, limit: Option<usize>) -> Result<Vec<crate::domain::audit::AuditEntry>, NodError>;
        }
    }

    fn dummy_params<'a>() -> LifecycleParams<'a> {
        LifecycleParams {
            target: None,
            tag: None,
            role: None,
            all: false,
            dry_run: false,
            concurrency: 4,
            strategy: "batch",
            batch_size: 0,
            fail_fast: false,
            auto_rollback: false,
            verbose: false,
            quiet: true,
        }
    }

    fn test_ctx(
        eval: MockFakeEvaluator,
        local: MockFakeDeployer,
        ssh: MockFakeDeployer,
        store: MockFakeConfigStore,
    ) -> AppContext {
        AppContext::new(Arc::new(eval), Arc::new(local), Arc::new(ssh))
            .with_config_store(Arc::new(store))
    }

    #[tokio::test]
    async fn lifecycle_rejects_zero_concurrency() {
        let ctx = test_ctx(
            MockFakeEvaluator::new(),
            MockFakeDeployer::new(),
            MockFakeDeployer::new(),
            MockFakeConfigStore::new(),
        );
        let mut params = dummy_params();
        params.concurrency = 0;

        let err = execute_lifecycle(ctx, Path::new("."), DeploymentAction::Switch, params)
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        assert!(err.to_string().contains("--concurrency must be at least 1"));
    }

    #[tokio::test]
    async fn lifecycle_rejects_unknown_strategy() {
        let ctx = test_ctx(
            MockFakeEvaluator::new(),
            MockFakeDeployer::new(),
            MockFakeDeployer::new(),
            MockFakeConfigStore::new(),
        );
        let mut params = dummy_params();
        params.strategy = "nonexistent-strategy";

        let err = execute_lifecycle(ctx, Path::new("."), DeploymentAction::Switch, params)
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        assert!(err.to_string().contains("unknown rollout strategy"));
    }

    #[tokio::test]
    async fn lifecycle_unmatched_target_returns_error() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("jello", "127.0.0.1", false)]));

        let ctx = test_ctx(
            eval,
            MockFakeDeployer::new(),
            MockFakeDeployer::new(),
            MockFakeConfigStore::new(),
        );
        let mut params = dummy_params();
        params.target = Some("nonexistent_host");

        let err = execute_lifecycle(ctx, Path::new("."), DeploymentAction::Switch, params)
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        assert!(err.to_string().contains("no hosts matched"));
    }

    #[tokio::test]
    async fn lifecycle_happy_path_runs_deploy() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("jello", "jello-machine", true)]));
        eval.expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/test-system")));

        let mut local = MockFakeDeployer::new();
        local.expect_check_reachability().returning(|_| Ok(true));
        local
            .expect_deploy_and_activate()
            .returning(|_, _, _, _, _| Ok(()));

        let mut store = MockFakeConfigStore::new();
        store.expect_apply_to().returning(Ok);
        store
            .expect_resolve()
            .returning(|h| Ok(SshProfile::for_host(h)));

        let ctx = test_ctx(eval, local, MockFakeDeployer::new(), store);
        let mut params = dummy_params();
        params.target = Some("jello");

        let res = execute_lifecycle(ctx, Path::new("."), DeploymentAction::Switch, params).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn lifecycle_happy_path_records_audit_outcome() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("jello", "jello-machine", true)]));
        eval.expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/test-system")));

        let mut local = MockFakeDeployer::new();
        local.expect_check_reachability().returning(|_| Ok(true));
        local
            .expect_deploy_and_activate()
            .returning(|_, _, _, _, _| Ok(()));

        let mut store = MockFakeConfigStore::new();
        store.expect_apply_to().returning(Ok);
        store
            .expect_resolve()
            .returning(|h| Ok(SshProfile::for_host(h)));

        let mut audit = MockFakeAuditStore::new();
        audit
            .expect_record()
            .with(
                mockall::predicate::eq("jello"),
                mockall::predicate::eq("completed"),
            )
            .times(1)
            .returning(|_, _| Ok(()));

        let ctx =
            test_ctx(eval, local, MockFakeDeployer::new(), store).with_audit_store(Arc::new(audit));
        let mut params = dummy_params();
        params.target = Some("jello");

        let res = execute_lifecycle(ctx, Path::new("."), DeploymentAction::Switch, params).await;
        assert!(res.is_ok());
    }
}
