//! `nod reboot` command: safe orchestrated host reboots with recovery verification (ADR-017).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::reboot_fleet::{RebootFleetUseCase, RebootOptions};
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    options: RebootOptions,
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

    let use_case = RebootFleetUseCase::new(Arc::new(ctx));
    let summary = use_case.execute(targets, options).await?;

    println!(
        "\n{:<22} {:<12} {:<12} {}",
        "HOST".bold(),
        "STATUS".bold(),
        "ELAPSED".bold(),
        "DETAILS".bold()
    );
    for outcome in &summary.outcomes {
        let status = if outcome.ok {
            "✓ rebooted".green()
        } else {
            "✗ failed".red()
        };
        let elapsed_str = format!("{:.1}s", outcome.elapsed.as_secs_f64());
        let details = outcome.failure.as_deref().unwrap_or("-");

        println!(
            "{:<22} {:<12} {:<12} {}",
            outcome.host_name.bold(),
            status,
            elapsed_str.dimmed(),
            details.dimmed()
        );
    }

    println!(
        "\n  {}",
        format!(
            "{} succeeded, {} failed",
            summary.succeeded(),
            summary.failed()
        )
        .dimmed()
    );

    if summary.failed() > 0 {
        return Err(NodError::deployment(format!(
            "{} host(s) failed during reboot",
            summary.failed()
        )));
    }

    Ok(())
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
            async fn reboot(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
        }
    }

    #[tokio::test]
    async fn reboot_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let mut deployer = MockFakeDeployer::new();
        deployer.expect_reboot().returning(|_, _| Ok(()));
        deployer.expect_check_reachability().returning(|_| Ok(true));

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(deployer),
            Arc::new(MockFakeDeployer::new()),
        );

        let options = RebootOptions {
            wait: false,
            ..Default::default()
        };

        let res = execute(ctx, Path::new("."), &TargetArgs::default(), options, false).await;
        assert!(res.is_ok());
    }
}
