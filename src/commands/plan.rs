//! `nod plan` command: build a deployment preview WITHOUT activating any host
//! (ADR-003 planning stage). Backed by `GeneratePlanUseCase`.

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::generate_plan::GeneratePlanUseCase;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};

pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

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

    let options = DeploymentOptions {
        dry_run: true,
        concurrency: 1,
        strategy: RolloutStrategy::All,
        batch_size: 0,
        fail_fast: false,
        auto_rollback: false,
        action: DeploymentAction::Switch,
        verbose,
        out_link: None,
        builder: None,
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
                "changed".to_string().yellow()
            } else {
                "unchanged".to_string().dimmed()
            };
            println!("  - {}: {}", diff.host_name, flag);
        }
    }

    Ok(())
}
