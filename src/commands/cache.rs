//! `nod cache` command: push compiled system closures to remote binary caches (ADR-019).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::push_cache::PushCacheUseCase;
use crate::config::options::TargetArgs;
use crate::domain::cache::CachePushOptions;
use crate::domain::errors::NodError;

pub async fn execute_push(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    options: CachePushOptions,
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

    let use_case = PushCacheUseCase::new(Arc::new(ctx));
    let reports = use_case
        .execute(&targets, flake_path, &options, verbose)
        .await?;

    if json {
        println!("{}", serde_json::to_string(&reports).unwrap());
        return Ok(());
    }

    println!(
        "\n{:<20} {:<12} {:<30} {}",
        "HOST".bold(),
        "STATUS".bold(),
        "CACHE URI".bold(),
        "CLOSURE PATH".bold()
    );

    let mut failed = 0;
    for r in &reports {
        let status = if r.ok {
            "✓ pushed".green()
        } else {
            failed += 1;
            "✗ failed".red()
        };

        println!(
            "{:<20} {:<12} {:<30} {}",
            r.host_name.bold(),
            status,
            r.cache_uri.cyan(),
            r.closure_path.display().to_string().dimmed()
        );

        if let Some(err) = &r.error {
            println!("      {}", err.red().dimmed());
        }
    }

    if failed > 0 {
        return Err(NodError::deployment(format!(
            "{} host(s) failed during cache push",
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
    async fn cache_push_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));
        eval.expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/test-closure")));

        let mut store_mock = MockFakeStorePort::new();
        store_mock
            .expect_push_cache()
            .returning(|host, _, closure, uri| {
                Ok(crate::domain::cache::CachePushReport {
                    host_name: host.name.clone(),
                    closure_path: closure.to_path_buf(),
                    cache_uri: uri.to_string(),
                    ok: true,
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

        let options = CachePushOptions {
            cache_uri: Some("s3://test-cache".to_string()),
            dry_run: false,
            concurrency: 1,
        };

        let res = execute_push(
            ctx,
            Path::new("."),
            &TargetArgs::default(),
            options,
            false,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
