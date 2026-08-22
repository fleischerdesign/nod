use crate::domain::host::{HostEntity, HostRole};
use crate::domain::traits::evaluator::NixEvaluator;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;
use tokio::process::Command;

pub struct NixCliEvaluator;

impl NixCliEvaluator {
    pub fn new() -> Self {
        Self
    }

    fn create_braille_spinner(msg: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("  {spinner:.cyan} {msg}")
                .unwrap(),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    }
}

#[async_trait]
impl NixEvaluator for NixCliEvaluator {
    async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>> {
        let pb = Self::create_braille_spinner("Evaluating host matrix...");
        let start = Instant::now();

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
            .context("Failed to execute Nix evaluation for host discovery")?;

        pb.finish_and_clear();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Nix host discovery evaluation failed: {}", stderr));
        }

        let host_names: Vec<String> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse Nix host discovery JSON output")?;

        let mut hosts = Vec::new();
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();

        for name in host_names {
            let is_local = name == local_hostname;

            let target_host_expr = format!(
                "let c = (import {}).nixosConfigurations.{}.config; in if c ? deployment && c.deployment ? targetHost then c.deployment.targetHost else (if c ? networking && c.networking ? hostName then c.networking.hostName else \"{}\")",
                flake_path.display(),
                name,
                name
            );

            let host_target = Command::new("nix")
                .args(["eval", "--json", "--expr", &target_host_expr])
                .output()
                .await
                .ok()
                .and_then(|o| serde_json::from_slice::<String>(&o.stdout).ok())
                .unwrap_or_else(|| name.clone());

            let mut entity = HostEntity::new(&name, &host_target, is_local);
            entity.role = HostRole::Server;
            hosts.push(entity);
        }

        if verbose {
            println!("  {}", format!("Discovered {} hosts in {:?}", hosts.len(), start.elapsed()).dimmed());
        }

        Ok(hosts)
    }

    async fn build_toplevel(&self, flake_path: &Path, host_name: &str, verbose: bool) -> Result<PathBuf> {
        let pb = Self::create_braille_spinner(&format!("Building closure for {}...", host_name));

        let flake_attr = format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            flake_path.display(),
            host_name
        );

        let start = Instant::now();

        let output = Command::new("nix")
            .args(["build", "--json", &flake_attr, "--no-link"])
            .output()
            .await
            .context("Failed to build NixOS system closure")?;

        pb.finish_and_clear();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Nix build failed for host {}: {}", host_name, stderr));
        }

        let build_json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .context("Failed to parse Nix build JSON output")?;

        let out_path = build_json[0]["outputs"]["out"]
            .as_str()
            .ok_or_else(|| anyhow!("Build JSON did not contain 'out' store path"))?;

        if verbose {
            println!("  {}", format!("Build completed in {:?} -> {}", start.elapsed(), out_path).dimmed());
        }

        Ok(PathBuf::from(out_path))
    }
}
