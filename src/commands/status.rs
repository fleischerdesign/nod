//! `nod status` use case: live reachability matrix of discovered hosts.

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::domain::errors::NodError;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

pub async fn execute(
    flake_path: &Path,
    verbose: bool,
    tag: Option<&str>,
    role: Option<&str>,
) -> Result<(), NodError> {
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    );
    let evaluator = ctx.evaluator();

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;

    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let selected = TargetSelection::select_filtered(hosts, "all", &local_hostname, tag, role);

    if selected.is_empty() {
        println!("{}", "No hosts match the given filters.".yellow());
        return Ok(());
    }

    println!(
        "\n{:<14} {:<22} {:<10} {:<16} {:<10}",
        "HOST".bold(),
        "TARGET".bold(),
        "ROLE".bold(),
        "TAGS".bold(),
        "STATUS".bold()
    );

    for host in selected {
        let deployer = ctx.deployer_for(&host);
        let is_up = deployer
            .check_reachability(&host)
            .await
            .unwrap_or(false);
        let status_str = if is_up {
            "● Online".green()
        } else {
            "○ Offline".red()
        };
        let tags = if host.tags.is_empty() {
            "-".to_string()
        } else {
            let mut parts = String::new();
            let mut first = true;
            for t in host.tags {
                if first {
                    parts = t.to_string();
                    first = false;
                } else {
                    parts = format!("{}, {}", parts, t);
                }
            }
            parts
        };
        println!(
            "{:<14} {:<22} {:<10} {:<16} {:<10}",
            host.name, host.target_host, host.role.to_str(), tags, status_str
        );
    }

    Ok(())
}