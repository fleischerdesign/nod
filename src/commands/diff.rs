use crate::domain::traits::deployer::RemoteDeployer;
use crate::domain::traits::evaluator::NixEvaluator;
use crate::infrastructure::nix_evaluator::NixCliEvaluator;
use crate::infrastructure::tokio_ssh::TokioSshDeployer;
use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

pub async fn execute(target: &str, flake_path: &Path, verbose: bool) -> Result<()> {
    let evaluator = NixCliEvaluator::new();
    let deployer = TokioSshDeployer::new();

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;
    let target_host = hosts
        .into_iter()
        .find(|h| h.name == target || (target == "local" && h.is_local))
        .ok_or_else(|| anyhow!("Target host '{}' not found in flake nixosConfigurations.", target))?;

    println!("{}", format!("> Generating system diff preview for {}", target_host.name).bold().cyan());

    let new_closure = evaluator.build_toplevel(flake_path, &target_host.name, verbose).await?;

    if target_host.is_local {
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
        let is_up = deployer.check_reachability(&target_host).await.unwrap_or(false);
        if is_up {
            println!("  {}", format!("Remote host {} is online. Ready for closure diff.", target_host.name).dimmed());
        }
    }

    Ok(())
}
