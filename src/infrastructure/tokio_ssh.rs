use crate::domain::host::HostEntity;
use crate::domain::traits::deployer::RemoteDeployer;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use colored::Colorize;
use std::path::Path;
use std::time::Instant;
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
        let output = Command::new("ping")
            .args(["-c", "1", "-W", "2", &host.target_host])
            .output()
            .await;

        Ok(output.map(|o| o.status.success()).unwrap_or(false))
    }

    async fn deploy_and_activate(&self, host: &HostEntity, closure: &Path, verbose: bool) -> Result<()> {
        let start = Instant::now();

        if host.is_local {
            println!("  {}", "Activating local configuration...".dimmed());
            let switch_bin = closure.join("bin/switch-to-configuration");
            let status = Command::new("sudo")
                .args([switch_bin.to_str().unwrap(), "switch"])
                .status()
                .await
                .context("Failed to activate local NixOS configuration")?;

            if !status.success() {
                return Err(anyhow!("Local configuration activation failed."));
            }

            if verbose {
                println!("  {}", format!("Local activation finished in {:?}", start.elapsed()).dimmed());
            }
            return Ok(());
        }

        println!("  {}", format!("Copying closure to {} over SSH...", host.target_host).dimmed());
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

        println!("  {}", format!("Activating remote configuration on {}...", host.target_host).dimmed());
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

        if verbose {
            println!("  {}", format!("Remote deployment finished in {:?}", start.elapsed()).dimmed());
        }

        Ok(())
    }

    async fn rollback(&self, host: &HostEntity) -> Result<()> {
        println!("  {}", format!("Rolling back {}...", host.name).yellow());
        Ok(())
    }
}
