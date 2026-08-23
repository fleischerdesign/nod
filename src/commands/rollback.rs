//! `nod rollback` command: revert a host to its previous known-good generation
//! via `RollbackUseCase` (ADR-003).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::rollback::RollbackUseCase;
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

pub async fn execute(
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    cli_overrides: CliOverrides,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
) -> Result<(), NodError> {
    let config_store = TomlConfigStore::new(flake_path, cli_overrides)?;
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_config_store(Arc::new(config_store));
    let evaluator = ctx.evaluator();

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

    let host = targets[0].clone();
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
