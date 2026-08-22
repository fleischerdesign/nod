use crate::domain::traits::deployer::RemoteDeployer;
use crate::domain::traits::evaluator::NixEvaluator;
use crate::infrastructure::lix_evaluator::LixEvaluator;
use crate::infrastructure::tokio_ssh::TokioSshDeployer;
use anyhow::Result;
use colored::Colorize;
use std::path::Path;

pub async fn execute(flake_path: &Path) -> Result<()> {
    let evaluator = LixEvaluator::new();
    let deployer = TokioSshDeployer::new();

    println!("{}", "Discovering hosts from Nix Flake...".bold().cyan());
    let hosts = evaluator.discover_hosts(flake_path).await?;

    println!("\n{:<12} {:<15} {:<10}", "HOST".bold(), "TARGET IP".bold(), "STATUS".bold());
    println!("{}", "─".repeat(40));

    for host in hosts {
        let is_up = deployer.check_reachability(&host).await.unwrap_or(false);
        let status_str = if is_up {
            "● Online".bold().green()
        } else {
            "○ Offline".bold().red()
        };
        println!("{:<12} {:<15} {:<10}", host.name, host.target_host, status_str);
    }

    Ok(())
}
