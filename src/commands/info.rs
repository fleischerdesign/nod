//! `nod info` command: inspect comprehensive configuration and live host diagnostics (ADR-016).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::inspect_info::InspectInfoUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;

pub async fn execute(
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

    let use_case = InspectInfoUseCase::new(Arc::new(ctx));
    let host_infos = use_case.execute(&targets, flake_path).await?;

    if json {
        println!("{}", serde_json::to_string(&host_infos).unwrap());
        return Ok(());
    }

    for info in host_infos {
        println!("\n{}", format!("Host: {}", info.host_name).bold());
        println!("{:<24} {}", "Target Host:".bold(), info.target_host);
        println!(
            "{:<24} {}",
            "Type:".bold(),
            if info.is_local {
                "local".green()
            } else {
                "remote (SSH)".cyan()
            }
        );
        println!("{:<24} {}", "Role:".bold(), info.role);

        let tags_str = if info.tags.is_empty() {
            "-".to_string()
        } else {
            info.tags.join(", ")
        };
        println!("{:<24} {}", "Tags:".bold(), tags_str);

        println!(
            "{:<24} {}@{} (port: {})",
            "SSH Connection:".bold(),
            info.ssh_user,
            info.target_host,
            info.ssh_port
        );

        if let Some(builder) = info.builder {
            println!("{:<24} {}", "Remote Builder:".bold(), builder);
        }

        if let Some(gen) = info.active_generation {
            println!("{:<24} #{}", "Active Generation:".bold(), gen);
        }

        if let Some(health) = info.health_status {
            let color_health = if health == "running" {
                health.green()
            } else {
                health.yellow()
            };
            println!("{:<24} {}", "Systemd Status:".bold(), color_health);
        }

        if let Some(kernel) = info.kernel_version {
            println!("{:<24} {}", "Linux Kernel:".bold(), kernel);
        }

        if let Some(uptime) = info.uptime {
            println!("{:<24} {}", "Uptime:".bold(), uptime);
        }

        if let Some(ref current) = info.current_closure {
            println!(
                "{:<24} {}",
                "Current System:".bold(),
                current.display().to_string().dimmed()
            );
        }

        if let Some(ref booted) = info.booted_closure {
            let matches_current = info.current_closure.as_ref() == Some(booted);
            let state = if matches_current {
                " (synced with current)".green()
            } else {
                " (reboot pending)".yellow()
            };
            println!(
                "{:<24} {}{}",
                "Booted System:".bold(),
                booted.display().to_string().dimmed(),
                state
            );
        }
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
        }
    }

    #[tokio::test]
    async fn info_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let mut deployer = MockFakeDeployer::new();
        deployer.expect_check_reachability().returning(|_| Ok(true));
        deployer.expect_current_closure().returning(|_, _| Ok(None));

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(deployer),
            Arc::new(MockFakeDeployer::new()),
        );

        let res = execute(ctx, Path::new("."), &TargetArgs::default(), false, false).await;
        assert!(res.is_ok());
    }
}
