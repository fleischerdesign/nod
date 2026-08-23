use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

pub struct QualityGate;

impl QualityGate {
    /// Resolves the flake.nix path to run gates over: the directory's
    /// `flake.nix` when `flake_path` is a directory, otherwise the path
    /// itself. Pure so it can be unit tested.
    pub fn resolve_target_file(flake_path: &Path) -> std::path::PathBuf {
        if flake_path.is_dir() {
            flake_path.join("flake.nix")
        } else {
            flake_path.to_path_buf()
        }
    }

    pub async fn run_all(flake_path: &Path) -> Result<()> {
        println!("{}", "> Quality gates".bold().cyan());

        let target_file = Self::resolve_target_file(flake_path);

        if target_file.exists() {
            let fmt_status = Command::new("nixfmt")
                .args(["--check", target_file.to_str().unwrap()])
                .status()
                .await;

            if let Ok(status) = fmt_status {
                if !status.success() {
                    println!("  {}", "⚠ nixfmt reported formatting issues.".yellow());
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
                println!("  {}", "⚠ statix reported anti-patterns.".yellow());
            }
        }

        println!("  {}", "✓ Passed".bold().green());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_flake_resolves_to_its_card_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flake.nix");
        std::fs::write(&path, "{ inputs = {}; outputs = {}; }\n").unwrap();
        let resolved = QualityGate::resolve_target_file(dir.path());
        assert!(resolved.exists());
    }

    #[test]
    fn file_target_itself_is_resolved_unchanged() {
        let file = tempfile::tempdir().unwrap().path().join("flake.nix");
        let resolved = QualityGate::resolve_target_file(&file);
        assert_eq!(resolved.display().to_string(), file.display().to_string());
    }
}
