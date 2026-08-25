//! `nod watch` command: live auto-preview rebuild and plan diff on file changes (ADR-022).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::watch_flake::WatchFlakeUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;
use crate::domain::watch::WatchOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    options: WatchOptions,
    verbose: bool,
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

    println!(
        "\n{} Watching {} for changes (poll: {}s, targets: {})...",
        "👁".cyan(),
        flake_path.display().to_string().bold(),
        options.poll_interval_secs,
        targets.len()
    );

    let use_case = WatchFlakeUseCase::new(Arc::new(ctx));
    let mut last_mtime = WatchFlakeUseCase::compute_max_mtime(flake_path);

    // Initial evaluation pass
    if let Ok(plan) = use_case.evaluate_pass(&targets, flake_path, verbose).await {
        for target in plan.targets {
            let closure_str = target
                .new_closure
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!(
                "  • Host {}: closure {}",
                target.host_name.bold(),
                closure_str.dimmed()
            );
        }
    }

    loop {
        sleep(Duration::from_secs(options.poll_interval_secs)).await;

        let current_mtime = WatchFlakeUseCase::compute_max_mtime(flake_path);
        if current_mtime != last_mtime {
            last_mtime = current_mtime;
            println!(
                "\n{} File modification detected. Re-evaluating plan...",
                "↻".yellow().bold()
            );

            match use_case.evaluate_pass(&targets, flake_path, verbose).await {
                Ok(plan) => {
                    for target in plan.targets {
                        let closure_str = target
                            .new_closure
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        println!(
                            "  ✓ Host {}: closure {}",
                            target.host_name.green().bold(),
                            closure_str.dimmed()
                        );
                    }
                }
                Err(e) => {
                    println!("  ✗ Evaluation failed: {}", e.to_string().red());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{HostEntity, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
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

    #[tokio::test]
    async fn watch_computes_targets() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        );

        let evaluator = ctx.evaluator();
        let hosts = evaluator
            .discover_hosts(Path::new("."), false)
            .await
            .unwrap();
        assert_eq!(hosts.len(), 1);
    }
}
