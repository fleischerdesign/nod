//! Local deployer adapter: `sudo switch-to-configuration` on this machine.
//!
//! The reachability probe never invokes the SSH transport (ADR-001 spec:
//! "reachability probe routing").

use async_trait::async_trait;
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::ports::deployer::DeployerPort;

/// Deploys local hosts through `sudo <closure>/bin/switch-to-configuration`.
pub struct LocalDeployer;

impl LocalDeployer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalDeployer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeployerPort for LocalDeployer {
    async fn check_reachability(&self, _host: &HostEntity) -> Result<bool, NodError> {
        // A local host does not need an SSH transport probe; it is always
        // "reachable" from the machine it runs on.
        Ok(true)
    }

    async fn current_closure(&self, _host: &HostEntity) -> Result<Option<PathBuf>, NodError> {
        // The live local closure is the `/run/current-system` link. Reading
        // the link so the store path is comparable to a fresh `nix build`
        // output (both resolve under `/nix/store/...`).
        let current = Path::new("/run/current-system");
        if !current.exists() {
            return Ok(None);
        }
        let resolved = std::fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
        Ok(Some(resolved))
    }

    async fn deploy_and_activate(
        &self,
        host: &HostEntity,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError> {
        let start = Instant::now();
        println!("  {}", "Activating local configuration...".dimmed());

        let switch_bin = closure.join("bin/switch-to-configuration");
        let status = Command::new("sudo")
            .args([switch_bin.to_str().unwrap(), action])
            .status()
            .await;

        if status.is_err() {
            return Err(NodError::local_activate("failed to launch sudo switch-to-configuration"));
        }
        if !status.unwrap().success() {
            return Err(NodError::local_activate("switch-to-configuration reported failure"));
        }

        if verbose {
            println!(
                "  {}",
                format!("Local activation finished in {:?}", start.elapsed()).dimmed()
            );
        }
        let _ = host;
        Ok(())
    }

    async fn rollback(&self, host: &HostEntity) -> Result<(), NodError> {
        println!(
            "  {}",
            format!("Rolling back local host {} to previous generation...", host.name).yellow()
        );
        // Re-invoke the prior generation's profile or ask nixos-rebuild to
        // switch back to the previous known-good configuration (ADR-003).
        let status = Command::new("nixos-rebuild")
            .args(["--rollback", "switch"])
            .status()
            .await;
        if status.is_err() {
            return Err(NodError::rollback_failure(
                "failed to launch `nixos-rebuild --rollback`"
            ));
        }
        if !status.unwrap().success() {
            return Err(NodError::rollback_failure(
                "nixos-rebuild --rollback reported failure"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristics() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalDeployer>();
    }

    #[tokio::test]
    async fn local_reachability_does_not_use_ssh() {
        let deployer = LocalDeployer::new();
        let host = HostEntity::new("jello", "jello-machine", true);
        let is_up = deployer.check_reachability(&host).await.unwrap();
        assert!(is_up);
    }
}