use crate::domain::traits::deployer::RemoteDeployer;
use crate::domain::traits::evaluator::NixEvaluator;
use crate::infrastructure::nix_evaluator::NixCliEvaluator;
use crate::infrastructure::tokio_ssh::TokioSshDeployer;
use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;

pub async fn execute(target: &str, flake_path: &Path, verbose: bool, quiet: bool) -> Result<()> {
    let evaluator = NixCliEvaluator::new();
    let deployer = TokioSshDeployer::new();

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;

    let targets = if target == "all" {
        hosts
    } else if target == "local" {
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();
        hosts.into_iter().filter(|h| h.name == local_hostname || h.is_local).collect()
    } else {
        hosts.into_iter().filter(|h| h.name == target).collect()
    };

    if targets.is_empty() {
        return Err(anyhow!("Target host '{}' not found in flake nixosConfigurations.", target));
    }

    for host in targets {
        if !quiet {
            println!("{}", format!("> Deploying {}", host.name).bold().cyan());
        }

        let closure = evaluator.build_toplevel(flake_path, &host.name, verbose).await?;
        if verbose {
            println!("  {}", format!("Closure: {}", closure.display()).dimmed());
        }

        deployer.deploy_and_activate(&host, &closure, verbose).await?;

        if !quiet {
            println!("  {}", format!("✓ Switched {}", host.name).bold().green());
        }
    }

    Ok(())
}
