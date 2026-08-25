//! `nod store` command: store optimization and maintenance across hosts (ADR-019).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::optimize_store::OptimizeStoreUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;

pub async fn execute_optimize(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    verbose: bool,
    json: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let hosts = evaluator
        .discover_hosts_degraded(flake_path, verbose)
        .await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let targets = resolve_targets(
        hosts,
        &local_hostname,
        target_args.target.as_deref(),
        target_args.tag.as_deref(),
        target_args.role.as_deref(),
        target_args.all,
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target_args.target.as_deref().unwrap_or("local"),
            target_args.tag.as_deref(),
            target_args.role.as_deref(),
        ));
    }

    let use_case = OptimizeStoreUseCase::new(Arc::new(ctx));
    let reports = use_case.execute(&targets).await?;

    if json {
        println!("{}", serde_json::to_string(&reports).unwrap());
        return Ok(());
    }

    println!(
        "\n{:<22} {:<12} {}",
        "HOST".bold(),
        "STATUS".bold(),
        "DETAILS".bold()
    );
    let mut failed = 0;
    for r in &reports {
        let status = if r.ok {
            "✓ optimized".green()
        } else {
            failed += 1;
            "✗ failed".red()
        };

        let details = r.error.as_deref().unwrap_or("-");
        println!(
            "{:<22} {:<12} {}",
            r.host_name.bold(),
            status,
            details.dimmed()
        );
    }

    if failed > 0 {
        return Err(NodError::deployment(format!(
            "{} host(s) failed during store optimization",
            failed
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cache::{CachePushReport, StoreOptimizeReport};
    use crate::domain::generation::{
        CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration,
    };
    use crate::domain::host::{HostEntity, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::store::StorePort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

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
        FakeStorePort {}
        #[async_trait]
        impl StorePort for FakeStorePort {
            async fn list_generations(&self, host: &HostEntity, profile: &SshProfile) -> Result<Vec<SystemGeneration>, NodError>;
            async fn collect_garbage(&self, host: &HostEntity, profile: &SshProfile, options: &GcOptions) -> Result<GcReport, NodError>;
            async fn copy_closure(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, options: &CopyOptions) -> Result<CopyReport, NodError>;
            async fn optimize_store(&self, host: &HostEntity, profile: &SshProfile) -> Result<StoreOptimizeReport, NodError>;
            async fn push_cache(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, cache_uri: &str) -> Result<CachePushReport, NodError>;
        }
    }

    #[tokio::test]
    async fn optimize_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let mut store_mock = MockFakeStorePort::new();
        store_mock.expect_optimize_store().returning(|host, _| {
            Ok(StoreOptimizeReport {
                host_name: host.name.clone(),
                ok: true,
                freed_bytes: None,
                error: None,
            })
        });

        let store_arc = Arc::new(store_mock);
        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_stores(store_arc.clone(), store_arc);

        let res = execute_optimize(ctx, Path::new("."), &TargetArgs::default(), false, false).await;
        assert!(res.is_ok());
    }
}
