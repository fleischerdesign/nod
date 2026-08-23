//! `nod diff` use case: package & systemd unit diff preview before switching.

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
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
    let store = ctx.config_store()?;

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

    for host in targets {
        // Materialize merged TOML/CLI user+port onto the entity so the
        // deployer's resolved profile uses them (ADR-004).
        let host = store.apply_to(host).await?;

        println!(
            "{}",
            format!("> Generating system diff preview for {}", host.name)
                .bold()
                .cyan()
        );

        let new_closure = evaluator
            .build_toplevel(flake_path, &host.name, None, verbose)
            .await?;

        if host.is_local {
            let current_closure = Path::new("/run/current-system");
            if current_closure.exists() {
                println!(
                    "  {}",
                    format!("Comparing /run/current-system vs {}", new_closure.display()).dimmed()
                );

                let nvd_status = Command::new("nvd")
                    .args(["diff", "/run/current-system", new_closure.to_str().unwrap()])
                    .status()
                    .await;

                if nvd_status.is_err() || !nvd_status.unwrap().success() {
                    // Fallback to nix store diff-closures if nvd is not available
                    let _ = Command::new("nix")
                        .args([
                            "store",
                            "diff-closures",
                            "/run/current-system",
                            new_closure.to_str().unwrap(),
                        ])
                        .status()
                        .await;
                }
            }
        } else {
            let deployer = ctx.deployer_for(&host);
            let is_up = deployer.check_reachability(&host).await.unwrap_or(false);
            if is_up {
                println!(
                    "  {}",
                    format!(
                        "Remote host {} is online. Ready for closure diff.",
                        host.name
                    )
                    .dimmed()
                );
            }
        }
    }

    Ok(())
}
