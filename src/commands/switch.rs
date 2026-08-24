//! `nod switch` command: rebuild and deploy Nix configurations for one host or
//! a fleet. Delegates rollout to `DeployFleetUseCase` (ADR-005).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::deploy_fleet::DeployFleetUseCase;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    quiet: bool,
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
    let targets = resolve_targets(
        hosts,
        &local_hostname,
        target,
        tag,
        role,
        all,
        DefaultScope::Local,
    );

    if targets.is_empty() {
        // Deploy commands default to local, so name that in the empty-set
        // error (consistent with the pre-ADR-008 message for a bare run).
        return Err(TargetSelection::unmatched(
            target.unwrap_or("local"),
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
        crate::commands::render_summary(&summary, verbose);
    }

    Ok(())
}
