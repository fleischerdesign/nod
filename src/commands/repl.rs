//! `nod repl` command: launch an interactive REPL with host configuration pre-loaded (ADR-016).

use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target: Option<&str>,
    verbose: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let hosts = evaluator
        .discover_hosts_degraded(flake_path, verbose)
        .await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let targets = resolve_targets(
        hosts,
        &local_hostname,
        target,
        None,
        None,
        false,
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target.unwrap_or("local"),
            None,
            None,
        ));
    }

    let host = &targets[0];
    let abs_flake = std::fs::canonicalize(flake_path).unwrap_or_else(|_| flake_path.to_path_buf());
    let expr = format!(
        "let flake = builtins.getFlake \"{}\"; host = flake.nixosConfigurations.{}; in {{ inherit flake host; inherit (host) config options pkgs; }}",
        abs_flake.display(),
        host.name
    );

    println!(
        "{}",
        format!(
            "Starting nix repl for host '{}' (loaded: flake, host, config, options, pkgs)...",
            host.name
        )
        .dimmed()
    );

    let status = Command::new("nix")
        .args(["repl", "--impure", "--expr", &expr])
        .status()
        .await
        .map_err(|e| NodError::internal(format!("failed to spawn nix repl: {e}")))?;

    if !status.success() {
        return Err(NodError::internal("nix repl exited with non-zero status"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{HostEntity, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
        }
    }

    #[test]
    fn repl_module_compiles() {
        let _ = true;
    }
}
