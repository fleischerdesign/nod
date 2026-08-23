//! `nod plan` command: build a deployment preview WITHOUT activating any host
//! (ADR-003 planning stage). Backed by `GeneratePlanUseCase`.

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::generate_plan::GeneratePlanUseCase;
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

pub async fn execute(
    target: &str,
    flake_path: &Path,
    verbose: bool,
    cli_overrides: CliOverrides,
    tag: Option<&str>,
    role: Option<&str>,
) -> Result<(), NodError> {
    let config_store = TomlConfigStore::new(flake_path, cli_overrides)?;
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_config_store(Arc::new(config_store));

    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let targets = TargetSelection::select_filtered(hosts, target, &local_hostname, tag, role);

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(target, tag, role));
    }

    let mut staged = Vec::<HostEntity>::with_capacity(targets.len());
    for host in targets {
        staged.push(store.apply_to(host).await?);
    }

    let options = DeploymentOptions {
        dry_run: true,
        concurrency: 1,
        strategy: RolloutStrategy::All,
        batch_size: 0,
        fail_fast: false,
        auto_rollback: false,
        action: DeploymentAction::Switch,
        verbose,
    };

    let use_case = GeneratePlanUseCase::new(Arc::new(ctx));
    let plan = use_case.plan(staged, options, flake_path).await?;

    println!("{}", format!("{} staged target(s)", plan.size()).bold());
    for target_plan in plan.targets.clone() {
        let closure = target_plan
            .new_closure
            .clone()
            .map(|p| p.display().to_string())
            .unwrap_or("—".to_string());
        println!(
            "  {} {} {}",
            target_plan.host_name.bold(),
            format!("[{}]", target_plan.action.to_str()).dimmed(),
            closure
        );
    }

    let diffs = use_case.diffs(plan.clone());
    if !diffs.is_empty() {
        println!("\n{}", "Closure diffs (preview):".bold());
        for diff in diffs {
            let flag = if diff.changed {
                format!("changed").yellow()
            } else {
                format!("unchanged").dimmed()
            };
            println!("  - {}: {}", diff.host_name, flag);
        }
    }

    Ok(())
}