//! `nod rollback` command: revert a host to its previous known-good generation
//! via `RollbackUseCase` (ADR-003).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::rollback::RollbackUseCase;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();

    let hosts = evaluator.discover_hosts_strict(flake_path, verbose).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    // Rollback is single-host (ADR-003 reverts one target via
    // `nixos-rebuild --rollback switch`). `resolve_targets` applies the
    // default/filters, then `select_exact_one` rejects both an empty match
    // and a multi-match instead of silently operating on a subset of the
    // fleet (audit B3).
    let resolved = resolve_targets(
        hosts,
        &local_hostname,
        target,
        tag,
        role,
        all,
        DefaultScope::Local,
    );
    let host =
        TargetSelection::select_exact_one(resolved, None, None, None, false, &local_hostname)?;
    println!(
        "{}",
        format!("> Rolling back {}", host.name).bold().yellow()
    );

    let audit_store_res = ctx.audit_store();
    let use_case = RollbackUseCase::new(Arc::new(ctx));
    use_case.execute(&host).await?;

    if let Ok(audit_store) = audit_store_res {
        if let Err(e) = audit_store.record(&host.name, "rolled_back").await {
            tracing::warn!(
                "Failed to record rollback audit entry for {}: {}",
                host.name,
                e
            );
        }
    }

    println!(
        "  {}",
        format!("✓ Rollback of {} completed successfully.", host.name).green()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{BuilderHost, HostEntity, SshProfile};
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
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    #[tokio::test]
    async fn rollback_rejects_multi_match_fleet() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts().returning(|_, _| {
            Ok(vec![
                HostEntity::new("web-01", "10.0.0.1", false),
                HostEntity::new("web-02", "10.0.0.2", false),
            ])
        });

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        );

        let err = execute(ctx, Some("web-*"), Path::new("."), false, None, None, false)
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    mock! {
        FakeAuditStore {}
        #[async_trait]
        impl crate::domain::ports::audit_store::AuditStorePort for FakeAuditStore {
            async fn record(&self, host_name: &str, outcome: &str) -> Result<(), NodError>;
            async fn entries(&self, host: Option<String>, limit: Option<usize>) -> Result<Vec<crate::domain::audit::AuditEntry>, NodError>;
        }
    }

    #[tokio::test]
    async fn rollback_single_host_succeeds() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("rollins", "100.126.5.72", true)]));

        let mut local = MockFakeDeployer::new();
        local.expect_rollback().returning(|_, _| Ok(()));

        let mut audit = MockFakeAuditStore::new();
        audit
            .expect_record()
            .with(
                mockall::predicate::eq("rollins"),
                mockall::predicate::eq("rolled_back"),
            )
            .times(1)
            .returning(|_, _| Ok(()));

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(local),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_audit_store(Arc::new(audit));

        let res = execute(
            ctx,
            Some("rollins"),
            Path::new("."),
            false,
            None,
            None,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
