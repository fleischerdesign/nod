//! `nod test` command: run `switch-to-configuration test` on one host or a
//! fleet without fully activating the new configuration. Delegates rollout to
//! `DeployFleetUseCase` (ADR-005, ADR-006 lifecycle commands).

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
    flake_path: Option<&Path>,
    verbose: bool,
    quiet: bool,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    concurrency: Option<usize>,
    strategy: Option<&str>,
    batch_size: Option<usize>,
    fail_fast: bool,
    auto_rollback: bool,
) -> Result<(), NodError> {
    let flake_path = flake_path.unwrap_or_else(|| Path::new("."));
    let concurrency = concurrency.unwrap_or(4);
    if concurrency == 0 {
        return Err(NodError::config("--concurrency must be at least 1"));
    }
    let strategy = strategy.unwrap_or("batch");
    let batch_size = batch_size.unwrap_or(0);

    let parsed_strategy = RolloutStrategy::parse(strategy);
    if parsed_strategy.is_none() {
        return Err(NodError::config(format!(
            "unknown rollout strategy '{strategy}'"
        )));
    }

    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    let options = DeploymentOptions {
        dry_run: false,
        concurrency,
        strategy: parsed_strategy.unwrap(),
        batch_size,
        fail_fast,
        auto_rollback,
        action: DeploymentAction::Test,
        verbose,
        out_link: None,
        builder: None,
    };

    let hosts = evaluator
        .discover_hosts_degraded(flake_path, verbose)
        .await?;
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
        return Err(TargetSelection::unmatched(
            target.unwrap_or("local"),
            tag,
            role,
        ));
    }

    let mut staged = Vec::<HostEntity>::with_capacity(targets.len());
    for host in targets {
        staged.push(store.apply_to(host).await?);
    }

    let use_case = DeployFleetUseCase::new(Arc::new(ctx));
    let summary = use_case.execute(staged, options, flake_path).await?;

    if !quiet {
        crate::commands::render_summary(&summary);
    }

    Ok(())
}
