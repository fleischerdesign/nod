use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

pub struct QualityGate;

impl QualityGate {
    pub async fn run_all(flake_path: &Path) -> Result<()> {
        println!("{}", "Running repository quality gates...".bold().cyan());

        let target_file = if flake_path.is_dir() {
            flake_path.join("flake.nix")
        } else {
            flake_path.to_path_buf()
        };

        if target_file.exists() {
            let fmt_status = Command::new("nixfmt")
                .args(["--check", target_file.to_str().unwrap()])
                .status()
                .await;

            if let Ok(status) = fmt_status {
                if !status.success() {
                    println!("{}", "⚠ nixfmt check reported formatting issues.".yellow());
                }
            }
        }

        let deadnix_status = Command::new("deadnix")
            .args(["--fail", &flake_path.display().to_string()])
            .status()
            .await;

        if let Ok(status) = deadnix_status {
            if !status.success() {
                return Err(anyhow!("deadnix check failed (dead code detected)."));
            }
        }

        let statix_status = Command::new("statix")
            .args(["check", &flake_path.display().to_string()])
            .status()
            .await;

        if let Ok(status) = statix_status {
            if !status.success() {
                println!("{}", "⚠ statix check reported anti-patterns.".yellow());
            }
        }

        println!("{}", "✓ Quality gates passed successfully!".bold().green());
        Ok(())
    }
}
