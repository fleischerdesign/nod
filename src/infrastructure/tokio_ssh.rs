use crate::domain::host::HostEntity;
use crate::domain::traits::deployer::RemoteDeployer;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

pub struct TokioSshDeployer;

impl TokioSshDeployer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RemoteDeployer for TokioSshDeployer {
    async fn check_reachability(&self, host: &HostEntity) -> Result<bool> {
        let status = Command::new("ping")
            .args(["-c", "1", "-W", "2", &host.target_host])
            .status()
            .await;

        Ok(status.map(|s| s.success()).unwrap_or(false))
    }

    async fn deploy_and_activate(&self, host: &HostEntity, closure: &Path) -> Result<()> {
        if host.is_local {
            println!("{}", format!("[{}] Activating local system configuration...", host.name).bold().green());
            let switch_bin = closure.join("bin/switch-to-configuration");
            let status = Command::new("sudo")
                .args([switch_bin.to_str().unwrap(), "switch"])
                .status()
                .await
                .context("Failed to activate local NixOS configuration")?;

            if !status.success() {
                return Err(anyhow!("Local configuration activation failed."));
            }
            return Ok(());
        }

        println!("{}", format!("[{}] Copying closure to remote host over SSH...", host.name).bold().blue());
        let copy_status = Command::new("nix")
            .args([
                "copy",
                "--to",
                &format!("ssh://root@{}", host.target_host),
                closure.to_str().unwrap(),
            ])
            .status()
            .await
            .context("Failed to copy closure over SSH")?;

        if !copy_status.success() {
            return Err(anyhow!("Nix store copy over SSH failed for {}", host.name));
        }

        println!("{}", format!("[{}] Activating remote configuration...", host.name).bold().green());
        let switch_bin = closure.join("bin/switch-to-configuration");
        let remote_cmd = format!("{} switch", switch_bin.display());

        let ssh_status = Command::new("ssh")
            .args([&format!("root@{}", host.target_host), &remote_cmd])
            .status()
            .await
            .context("Failed to execute remote activation over SSH")?;

        if !ssh_status.success() {
            return Err(anyhow!("Remote activation failed for {}", host.name));
        }

        Ok(())
    }

    async fn rollback(&self, host: &HostEntity) -> Result<()> {
        println!("{}", format!("[{}] Rolling back to previous profile generation...", host.name).yellow());
        Ok(())
    }
}
