use crate::domain::traits::deployer::RemoteDeployer;
use crate::domain::traits::evaluator::NixEvaluator;
use crate::infrastructure::nix_evaluator::NixCliEvaluator;
use crate::infrastructure::tokio_ssh::TokioSshDeployer;
use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;
use std::time::Instant;

pub async fn execute(target: &str, flake_path: &Path, verbose: bool, quiet: bool) -> Result<()> {
    let evaluator = NixCliEvaluator::new();
    let deployer = TokioSshDeployer::new();

    let total_start = Instant::now();
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
        let host_start = Instant::now();
        if !quiet {
            println!("{}", format!("> Deploying {}", host.name).bold().cyan());
        }

        let eval_start = Instant::now();
        let closure = evaluator.build_toplevel(flake_path, &host.name, verbose).await?;
        let build_duration = eval_start.elapsed();

        if verbose {
            println!("  {}", format!("Closure: {}", closure.display()).dimmed());
        }

        let activate_start = Instant::now();
        deployer.deploy_and_activate(&host, &closure, verbose).await?;
        let activate_duration = activate_start.elapsed();

        let total_host_duration = host_start.elapsed();

        if !quiet {
            println!(
                "  {}",
                format!(
                    "⚡ Switched {} in {:.2?} (Build: {:.2?}, Activate: {:.2?})",
                    host.name, total_host_duration, build_duration, activate_duration
                )
                .bold()
                .green()
            );
        }
    }

    if verbose && !quiet {
        println!("\n  {}", format!("Total workflow finished in {:.2?}", total_start.elapsed()).dimmed());
    }

    Ok(())
}
