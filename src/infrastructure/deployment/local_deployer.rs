//! Local deployer adapter: `sudo switch-to-configuration` on this machine.
//!
//! The reachability probe never invokes the SSH transport (ADR-001 spec:
//! "reachability probe routing").

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};
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

    async fn current_closure(
        &self,
        _host: &HostEntity,
        _profile: &SshProfile,
    ) -> Result<Option<PathBuf>, NodError> {
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
        _profile: &SshProfile,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError> {
        let start = Instant::now();
        tracing::info!(host = %host.name, "Activating local configuration...");

        let switch_bin = closure.join("bin/switch-to-configuration");
        let status = Command::new("sudo")
            .args([switch_bin.to_str().unwrap(), action])
            .status()
            .await;

        if status.is_err() {
            return Err(NodError::local_activate(
                "failed to launch sudo switch-to-configuration",
            ));
        }
        if !status.unwrap().success() {
            return Err(NodError::local_activate(
                "switch-to-configuration reported failure",
            ));
        }

        if verbose {
            tracing::debug!(
                host = %host.name,
                elapsed = ?start.elapsed(),
                "Local activation finished"
            );
        }
        Ok(())
    }

    async fn rollback(&self, host: &HostEntity, _profile: &SshProfile) -> Result<(), NodError> {
        tracing::info!(
            host = %host.name,
            "Rolling back local host to previous generation..."
        );
        // Re-invoke the prior generation's profile or ask nixos-rebuild to
        // switch back to the previous known-good configuration (ADR-003).
        let status = Command::new("nixos-rebuild")
            .args(["--rollback", "switch"])
            .status()
            .await;
        if status.is_err() {
            return Err(NodError::rollback_failure(
                "failed to launch `nixos-rebuild --rollback`",
            ));
        }
        if !status.unwrap().success() {
            return Err(NodError::rollback_failure(
                "nixos-rebuild --rollback reported failure",
            ));
        }
        Ok(())
    }

    async fn reboot(&self, host: &HostEntity, _profile: &SshProfile) -> Result<(), NodError> {
        tracing::info!(host = %host.name, "Initiating local system reboot...");
        let status = Command::new("sudo")
            .args(["systemctl", "reboot"])
            .status()
            .await;
        if status.is_err() || !status.unwrap().success() {
            return Err(NodError::deployment(format!(
                "failed to execute local reboot for {}",
                host.name
            )));
        }
        Ok(())
    }
}

use crate::domain::generation::{CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration};
use crate::domain::ports::store::StorePort;

#[async_trait]
impl StorePort for LocalDeployer {
    async fn list_generations(
        &self,
        _host: &HostEntity,
        _profile: &SshProfile,
    ) -> Result<Vec<SystemGeneration>, NodError> {
        let output = Command::new("sh")
            .arg("-c")
            .arg("stat -c '%n %Y %N' /nix/var/nix/profiles/system* 2>/dev/null || true")
            .output()
            .await
            .map_err(|e| NodError::evaluation(format!("failed to stat system profiles: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_system_profiles_stat_output(&stdout))
    }

    async fn collect_garbage(
        &self,
        host: &HostEntity,
        _profile: &SshProfile,
        options: &GcOptions,
    ) -> Result<GcReport, NodError> {
        let is_root = std::env::var("USER").map(|u| u == "root").unwrap_or(false);
        let mut cmd = if options.dry_run || is_root {
            Command::new("nix-collect-garbage")
        } else {
            let mut c = Command::new("sudo");
            c.arg("nix-collect-garbage");
            c
        };

        if let Some(keep) = options.keep {
            if !options.dry_run {
                let mut env_cmd = if is_root {
                    Command::new("nix-env")
                } else {
                    let mut c = Command::new("sudo");
                    c.arg("nix-env");
                    c
                };
                let _ = env_cmd
                    .args([
                        "-p",
                        "/nix/var/nix/profiles/system",
                        "--delete-generations",
                        &format!("+{keep}"),
                    ])
                    .output()
                    .await;
            }
        } else if let Some(ref older_than) = options.older_than {
            cmd.args(["--delete-older-than", older_than]);
        } else if !options.dry_run {
            cmd.arg("-d");
        }

        if options.dry_run {
            cmd.arg("--dry-run");
        }

        let output = cmd.output().await.map_err(|e| {
            NodError::deployment(format!("failed to execute nix-collect-garbage: {e}"))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let success = output.status.success();
        let output_summary = if success {
            stdout.lines().last().unwrap_or("done").trim().to_string()
        } else {
            stderr.trim().to_string()
        };

        Ok(GcReport {
            host_name: host.name.clone(),
            success,
            output_summary,
        })
    }

    async fn copy_closure(
        &self,
        host: &HostEntity,
        _profile: &SshProfile,
        closure: &Path,
        options: &CopyOptions,
    ) -> Result<CopyReport, NodError> {
        let mut cmd = Command::new("nix");
        cmd.arg("copy");
        if let Some(ref to) = options.to {
            cmd.args(["--to", to]);
        }
        if let Some(ref from) = options.from {
            cmd.args(["--from", from]);
        }
        cmd.arg(closure.to_str().unwrap_or(""));

        let output = cmd
            .output()
            .await
            .map_err(|e| NodError::deployment(format!("failed to execute nix copy: {e}")))?;

        let success = output.status.success();
        Ok(CopyReport {
            host_name: host.name.clone(),
            closure_path: closure.to_path_buf(),
            success,
        })
    }
}

/// Parses the output of `stat -c "%n %Y %N" /nix/var/nix/profiles/system*` into structured generations.
pub fn parse_system_profiles_stat_output(output: &str) -> Vec<SystemGeneration> {
    let mut current_gen = None;

    // Pass 1: extract active generation from `/nix/var/nix/profiles/system` symlink target
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("/nix/var/nix/profiles/system ") {
            if let Some(idx) = trimmed.find("system-") {
                let rest = &trimmed[idx + 7..];
                if let Some(end) = rest.find("-link") {
                    if let Ok(gen_num) = rest[..end].parse::<u32>() {
                        current_gen = Some(gen_num);
                    }
                }
            }
        }
    }

    // Pass 2: parse each `system-<N>-link`
    let mut generations = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("/nix/var/nix/profiles/system-") {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let path_part = parts[0];
        let gen_str = path_part
            .trim_start_matches("/nix/var/nix/profiles/system-")
            .trim_end_matches("-link");
        let generation = match gen_str.parse::<u32>() {
            Ok(g) => g,
            Err(_) => continue,
        };

        let created_at = parts[1].parse::<u64>().ok();

        let target_part = if let Some(idx) = trimmed.find("->") {
            trimmed[idx + 2..]
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
        } else {
            ""
        };

        if target_part.is_empty() {
            continue;
        }

        let is_current = current_gen == Some(generation);

        generations.push(SystemGeneration {
            generation,
            is_current,
            created_at,
            closure_path: PathBuf::from(target_part),
        });
    }

    generations.sort_by_key(|g| std::cmp::Reverse(g.generation));
    generations
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_STAT_OUTPUT: &str = "\
/nix/var/nix/profiles/system 1787560497 '/nix/var/nix/profiles/system' -> 'system-120-link'
/nix/var/nix/profiles/system-119-link 1787559159 '/nix/var/nix/profiles/system-119-link' -> '/nix/store/r98640x8xsmgga8y8bc39xw4dk5lxg8z-nixos-system-yorke'
/nix/var/nix/profiles/system-120-link 1787560497 '/nix/var/nix/profiles/system-120-link' -> '/nix/store/8rc43cvqzvg01jj10lv2x0h6kwhly52y-nixos-system-yorke'";

    #[test]
    fn parse_system_profiles_stat_output_extracts_generations() {
        let gens = parse_system_profiles_stat_output(SAMPLE_STAT_OUTPUT);
        assert_eq!(gens.len(), 2);

        assert_eq!(gens[0].generation, 120);
        assert!(gens[0].is_current);
        assert_eq!(gens[0].created_at, Some(1787560497));
        assert_eq!(
            gens[0].closure_path,
            PathBuf::from("/nix/store/8rc43cvqzvg01jj10lv2x0h6kwhly52y-nixos-system-yorke")
        );

        assert_eq!(gens[1].generation, 119);
        assert!(!gens[1].is_current);
    }

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
