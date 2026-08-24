//! `nod rollback` command: revert a host to its previous known-good generation
//! via `RollbackUseCase` (ADR-003).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::rollback::RollbackUseCase;
use crate::domain::errors::NodError;

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

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    // Rollback is single-host (ADR-003 reverts one target via
    // `nixos-rebuild --rollback switch`). `resolve_targets` applies the
    // default/filters, then `select_exact_one` rejects both an empty match
    // and a multi-match instead of silently operating on a subset of the
    // fleet (audit B3).
    let resolved = resolve_targets(
        hosts,
        &local_hostname,
        target,
        tag,
        role,
        all,
        DefaultScope::Local,
    );
    let host =
        TargetSelection::select_exact_one(resolved, None, None, None, false, &local_hostname)?;
    println!(
        "{}",
        format!("> Rolling back {}", host.name).bold().yellow()
    );

    let use_case = RollbackUseCase::new(Arc::new(ctx));
    use_case.execute(&host).await?;

    if verbose {
        println!(
            "  {}",
            format!("Rollback of {} finished", host.name).dimmed()
        );
    }
    Ok(())
}
