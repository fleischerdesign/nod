//! `nod diff` use case: package & systemd unit diff preview before switching.

use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
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
