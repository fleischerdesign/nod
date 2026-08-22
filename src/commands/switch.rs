use crate::commands::check;
use crate::domain::traits::deployer::RemoteDeployer;
use crate::domain::traits::evaluator::NixEvaluator;
use crate::infrastructure::lix_evaluator::LixEvaluator;
use crate::infrastructure::tokio_ssh::TokioSshDeployer;
use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;

pub async fn execute(target: &str, no_check: bool, flake_path: &Path) -> Result<()> {
    if !no_check {
        check::execute(flake_path).await?;
    }

    let evaluator = LixEvaluator::new();
    let deployer = TokioSshDeployer::new();

    let hosts = evaluator.discover_hosts(flake_path).await?;

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
        println!("\n{}", format!("=== Deploying target: {} ===", host.name).bold().magenta());
        let closure = evaluator.build_toplevel(flake_path, &host.name).await?;
        println!("{}", format!("Built top-level closure: {}", closure.display()).dimmed());

        deployer.deploy_and_activate(&host, &closure).await?;
        println!("{}", format!("✓ Target {} switched successfully!", host.name).bold().green());
    }

    Ok(())
}
