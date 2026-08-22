use crate::domain::host::{HostEntity, HostRole};
use crate::domain::traits::evaluator::NixEvaluator;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub struct LixEvaluator;

impl LixEvaluator {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl NixEvaluator for LixEvaluator {
    async fn discover_hosts(&self, flake_path: &Path) -> Result<Vec<HostEntity>> {
        let output = Command::new("nix")
            .args([
                "eval",
                "--json",
                &format!("{}#nixosConfigurations", flake_path.display()),
                "--apply",
                "builtins.attrNames",
            ])
            .output()
            .await
            .context("Failed to execute Lix evaluation for host discovery")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Lix host discovery evaluation failed: {}", stderr));
        }

        let host_names: Vec<String> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse Lix host discovery JSON output")?;

        let mut hosts = Vec::new();
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();

        for name in host_names {
            let is_local = name == local_hostname;
            let mut entity = HostEntity::new(&name, &name, is_local);
            entity.role = HostRole::Server;
            hosts.push(entity);
        }

        Ok(hosts)
    }

    async fn build_toplevel(&self, flake_path: &Path, host_name: &str) -> Result<PathBuf> {
        let flake_attr = format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            flake_path.display(),
            host_name
        );

        let output = Command::new("nix")
            .args(["build", "--json", &flake_attr, "--no-link"])
            .output()
            .await
            .context("Failed to build NixOS system closure with Lix")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Lix build failed for host {}: {}", host_name, stderr));
        }

        let build_json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("Failed to parse Lix build JSON output")?;

        let out_path = build_json[0]["outputs"]["out"]
            .as_str()
            .ok_or_else(|| anyhow!("Build JSON did not contain 'out' store path"))?;

        Ok(PathBuf::from(out_path))
    }
}
