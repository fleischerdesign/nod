use crate::domain::traits::deployer::RemoteDeployer;
use crate::domain::traits::evaluator::NixEvaluator;
use crate::infrastructure::nix_evaluator::NixCliEvaluator;
use crate::infrastructure::tokio_ssh::TokioSshDeployer;
use anyhow::Result;
use colored::Colorize;
use std::path::Path;

pub async fn execute(flake_path: &Path, verbose: bool) -> Result<()> {
    let evaluator = NixCliEvaluator::new();
    let deployer = TokioSshDeployer::new();

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;

    println!("\n{:<14} {:<24} {:<10}", "HOST".bold(), "TARGET IP".bold(), "STATUS".bold());

    for host in hosts {
        let is_up = deployer.check_reachability(&host).await.unwrap_or(false);
        let status_str = if is_up {
            "● Online".green()
        } else {
            "○ Offline".red()
        };
        println!("{:<14} {:<24} {:<10}", host.name, host.target_host, status_str);
    }

    Ok(())
}
