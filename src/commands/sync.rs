//! `nod sync` command: pull-based GitOps synchronization (ADR-022).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::application::context::AppContext;
use crate::application::use_cases::sync_daemon::SyncDaemonUseCase;
use crate::domain::errors::NodError;
use crate::domain::watch::SyncOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    options: SyncOptions,
    _verbose: bool,
) -> Result<(), NodError> {
    let use_case = SyncDaemonUseCase::new(Arc::new(ctx));

    println!(
        "\n{} Starting GitOps reconciler for {} ({}/{})...",
        "🔄".cyan(),
        flake_path.display().to_string().bold(),
        options.remote.cyan(),
        options.branch.cyan()
    );

    if options.once {
        let report = use_case.reconcile_once(flake_path, &options).await?;
        if report.applied {
            println!(
                "{} Successfully synchronized to commit {}",
                "✓".green().bold(),
                report.commit_hash.cyan()
            );
        } else if let Some(err) = report.error {
            println!("{} Sync failed: {}", "✗".red().bold(), err.red());
        } else {
            println!(
                "{} Already up to date at commit {}",
                "✓".green(),
                report.commit_hash.dimmed()
            );
        }
        return Ok(());
    }

    loop {
        match use_case.reconcile_once(flake_path, &options).await {
            Ok(report) => {
                if report.applied {
                    println!(
                        "{} Successfully synchronized to commit {}",
                        "✓".green().bold(),
                        report.commit_hash.cyan()
                    );
                }
            }
            Err(e) => {
                println!(
                    "{} Reconciliation error: {}",
                    "✗".red().bold(),
                    e.to_string().red()
                );
            }
        }

        sleep(Duration::from_secs(options.interval_secs)).await;
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
    async fn sync_command_once_runs_successfully() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        );

        let opts = SyncOptions {
            dry_run: true,
            once: true,
            ..Default::default()
        };

        let res = execute(ctx, temp.path(), opts, false).await;
        assert!(res.is_ok());
    }
}
