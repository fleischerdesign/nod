//! SSH CLI deployer adapter: `nix copy --to` + `ssh ... switch` for remote
//! hosts. Connection parameters come from the derived `SshProfile`.

use async_trait::async_trait;
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::ports::deployer::DeployerPort;

/// Deploys remote hosts through store copy over SSH and a remote switch
/// activation.
pub struct SshCliDeployer;

impl SshCliDeployer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SshCliDeployer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeployerPort for SshCliDeployer {
    async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError> {
        let output = Command::new("ping")
            .args(["-c", "1", "-W", "2", &host.target_host])
            .output()
            .await;

        if output.is_err() {
            return Err(NodError::unreachable(host.name.clone()));
        }
        let output = output.unwrap();
        if !output.status.success() {
            return Err(NodError::unreachable(host.name.clone()));
        }
        Ok(true)
    }

    async fn current_closure(&self, host: &HostEntity) -> Result<Option<PathBuf>, NodError> {
        // Resolve the remote symlink over the same transport as activation;
        // a non-zero exit means the host has no live closure yet (or the
        // query failed), both of which surface as "no active closure".
        let profile = SshProfile::for_host(host);
        let ssh_target = format!("{}@{}", profile.user(), host.target_host);
        let output = Command::new("ssh")
            .args([&ssh_target, "readlink /run/current-system"])
            .output()
            .await;
        if output.is_err() {
            return Err(NodError::deployment(format!(
                "failed to query current closure of {} over SSH",
                host.name
            )));
        }
        let output = output.unwrap();
        if !output.status.success() {
            return Ok(None);
        }
        let path = String::from_utf8_lossy(&output.stdout);
        let path = path.trim();
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PathBuf::from(&path)))
        }
    }

    async fn deploy_and_activate(
        &self,
        host: &HostEntity,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError> {
        let profile = SshProfile::for_host(host);
        let start = Instant::now();

        println!(
            "  {}",
            format!(
                "Copying closure to {} over SSH...",
                host.target_host
            )
            .dimmed()
        );

        let store_target = format!("ssh://{}@{}", profile.user(), host.target_host);
        let copy_status = Command::new("nix")
            .args([
                "copy",
                "--to",
                &store_target,
                closure.to_str().unwrap(),
            ])
            .status()
            .await;

        if copy_status.is_err() {
            return Err(NodError::store_transfer(format!(
                "failed to launch `nix copy` for {}",
                host.name
            )));
        }
        if !copy_status.unwrap().success() {
            return Err(NodError::store_transfer(host.name.clone()));
        }

        println!(
            "  {}",
            format!(
                "Activating remote configuration on {}...",
                host.target_host
            )
            .dimmed()
        );

        let switch_bin = closure.join("bin/switch-to-configuration");
        let remote_cmd = format!("{} {}", switch_bin.display(), action);

        let ssh_target = format!("{}@{}", profile.user(), host.target_host);
        let ssh_status = Command::new("ssh")
            .args([&ssh_target, &remote_cmd])
            .status()
            .await;

        if ssh_status.is_err() {
            return Err(NodError::remote_activate(format!(
                "failed to launch `ssh` for {}",
                host.name
            )));
        }
        if !ssh_status.unwrap().success() {
            return Err(NodError::remote_activate(host.name.clone()));
        }

        if verbose {
            println!(
                "  {}",
                format!("Remote deployment finished in {:?}", start.elapsed()).dimmed()
            );
        }

        Ok(())
    }

    async fn rollback(&self, host: &HostEntity) -> Result<(), NodError> {
        let profile = SshProfile::for_host(host);
        let ssh_target = format!("{}@{}", profile.user(), host.target_host);
        println!(
            "  {}",
            format!("Rolling back remote host {} to previous generation...", host.name).yellow()
        );

        // Remote generation query: list the prior profile links available.
        let query = Command::new("ssh")
            .args([&ssh_target, "/nix/var/nix/profiles"])
            .status()
            .await;
        if query.is_err() {
            return Err(NodError::rollback_failure(format!(
                "failed to query remote generations for {}",
                host.name
            )));
        }
        if !query.unwrap().success() {
            return Err(NodError::rollback_failure(format!(
                "remote generation query failed for {}",
                host.name
            )));
        }

        // Roll back by switching to the previous known-good configuration.
        let remote_cmd = "nixos-rebuild --rollback switch".to_string();
        let rollback_status = Command::new("ssh")
            .args([&ssh_target, &remote_cmd])
            .status()
            .await;
        if rollback_status.is_err() {
            return Err(NodError::rollback_failure(format!(
                "failed to launch ssh rollback for {}",
                host.name
            )));
        }
        if !rollback_status.unwrap().success() {
            return Err(NodError::rollback_failure(host.name.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_stays_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SshCliDeployer>();
    }
}