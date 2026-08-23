//! `nod switch` command: rebuild and deploy Nix configurations for one host or
//! a fleet. Delegates rollout to `DeployFleetUseCase` (ADR-005).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::deploy_fleet::{DeployFleetUseCase, FleetSummary};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    quiet: bool,
    cli_overrides: CliOverrides,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    dry_run: bool,
    concurrency: usize,
    strategy: &str,
    batch_size: usize,
    fail_fast: bool,
    auto_rollback: bool,
    on_error: Option<&str>,
    action: &str,
) -> Result<(), NodError> {
    if concurrency == 0 {
        return Err(NodError::config("--concurrency must be at least 1"));
    }

    let config_store = TomlConfigStore::new(flake_path, cli_overrides)?;
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_config_store(Arc::new(config_store));

    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    // `--continue-on-error` is the inverse of `--fail-fast`.
    let continue_on_error = on_error.map(|mode| mode == "continue").unwrap_or(false);
    let effective_fail_fast = !continue_on_error && fail_fast;

    let parsed_strategy = RolloutStrategy::parse(strategy);
    if parsed_strategy.is_none() {
        return Err(NodError::config(format!(
            "unknown rollout strategy '{strategy}'"
        )));
    }

    let options = DeploymentOptions {
        dry_run,
        concurrency,
        strategy: parsed_strategy.unwrap(),
        batch_size,
        fail_fast: effective_fail_fast,
        auto_rollback,
        action: DeploymentAction::parse(action).unwrap_or(DeploymentAction::Switch),
        verbose,
        out_link: None,
        builder: None,
    };

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let (effective_target, effective_all) =
        if !all && target.is_none() && tag.is_none() && role.is_none() {
            (Some("local"), false)
        } else {
            (target, all)
        };
    let targets = TargetSelection::select(
        hosts,
        effective_target,
        tag,
        role,
        effective_all,
        &local_hostname,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            effective_target.unwrap_or("all"),
            tag,
            role,
        ));
    }

    // Materialize merged TOML/CLI user+port onto each staged host (ADR-004).
    let mut staged = Vec::<HostEntity>::with_capacity(targets.len());
    for host in targets {
        staged.push(store.apply_to(host).await?);
    }

    let use_case = DeployFleetUseCase::new(Arc::new(ctx));
    let summary = use_case.execute(staged, options, flake_path).await?;

    if !quiet {
        render_summary(&summary, verbose);
    }

    Ok(())
}

/// Renders the fleet result for the operator.
fn render_summary(summary: &FleetSummary, _verbose: bool) {
    for outcome in summary.outcomes.clone() {
        let label = format!("[{}]", outcome.state.to_str());
        let colored = if outcome.ok {
            label.green()
        } else {
            label.red()
        };
        println!("  {} {}", outcome.host_name.bold(), colored);
    }
    println!(
        "\n  {}",
        format!(
            "{} succeeded, {} rolled back, {} failed (aborted: {})",
            summary.succeeded(),
            summary.rolled_back(),
            summary.failed(),
            summary.aborted
        )
        .dimmed()
    );
}
