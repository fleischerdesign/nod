//! Local deployer adapter: `sudo switch-to-configuration` on this machine.
//!
//! The reachability probe never invokes the SSH transport (ADR-001 spec:
//! "reachability probe routing").

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::health::{DeploymentReport, HealthSnapshot, HealthVerdict};
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::ports::deployer::DeployerPort;
use crate::infrastructure::deployment::activation;

/// True when this process already holds root privileges.
///
/// `$USER` is unset in systemd and cron units and can lie under `su`, so the
/// effective uid is the only honest source. Reading it here is what keeps
/// `nod` working when it runs as a service instead of an interactive shell.
fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Builds a command that runs `program` directly as root and through `sudo`
/// otherwise. The elevation decision is a parameter, so it is testable without
/// actually holding either privilege.
fn privileged(program: &str, root: bool) -> Command {
    if root {
        Command::new(program)
    } else {
        let mut command = Command::new("sudo");
        command.arg(program);
        command
    }
}

/// Runs the activation, detached from this unit's control group when possible.
///
/// Activating the host that runs the daemon stops the daemon's own unit, and a
/// direct child would be killed with it. A root activation therefore goes
/// through a transient systemd unit; non-root or non-systemd hosts keep the
/// direct (sudo) call.
async fn activate(
    host: &HostEntity,
    switch_bin: &Path,
    action: &str,
    root: bool,
) -> std::io::Result<activation::Watched> {
    let program = switch_bin.to_str().unwrap_or_default();

    if root && Path::new("/run/systemd/system").is_dir() {
        let unit = format!(
            "nod-activate-{}-{}",
            sanitize_unit_name(&host.name),
            std::process::id()
        );
        let watched = activation::watch(Command::new("systemd-run").args([
            "--quiet",
            "--collect",
            "--wait",
            "--pipe",
            "--unit",
            &unit,
            "--property=Type=oneshot",
            program,
            action,
        ]))
        .await;
        if let Ok(watched) = watched {
            return Ok(watched);
        }
        // No systemd-run or it refused: fall back to the direct call.
    }

    let mut command = privileged(program, root);
    command.arg(action);
    activation::watch(&mut command).await
}

/// Maps a host name onto the character set systemd accepts in a unit name.
fn sanitize_unit_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Verifies the freshly activated local system against its baseline.
///
/// Returns the verdict; an `Err` means the probe itself could not run, which
/// the caller reports as `Unverified` rather than as a failed deployment.
async fn verify_local_health(
    host: &HostEntity,
    baseline: Option<&HealthSnapshot>,
    touched: &[String],
) -> Result<HealthVerdict, NodError> {
    use crate::domain::health::verdict_against;
    use crate::infrastructure::health::probe;

    let failed =
        run_local_probe(host, "systemctl", &["--failed", "--no-legend", "--plain"]).await?;
    let restarting = run_local_probe(host, "sh", &["-c", probe::RESTARTING_UNITS_COMMAND]).await?;
    let extra = match probe::extra_probe_command(host, touched) {
        Some(command) => run_local_probe(host, "sh", &["-c", &command]).await?,
        None => String::new(),
    };
    let current = probe::snapshot(host, &failed, &restarting, &extra);

    Ok(verdict_against(
        baseline,
        &current,
        probe::max_restarts(host),
    ))
}

/// Whether the running system asks for a reboot to complete the change.
fn reboot_required() -> bool {
    Path::new("/run/reboot-required").exists()
}

/// Observes the local host's health inputs, tolerating probe failures.
///
/// `None` means "could not observe", which the verdict treats as no baseline
/// rather than as a clean host.
async fn snapshot_local(host: &HostEntity) -> Option<HealthSnapshot> {
    use crate::infrastructure::health::probe;

    if !probe::enabled(host) {
        return Some(HealthSnapshot::clean());
    }

    let failed = run_local_probe(host, "systemctl", &["--failed", "--no-legend", "--plain"])
        .await
        .ok()?;
    let restarting = run_local_probe(host, "sh", &["-c", probe::RESTARTING_UNITS_COMMAND])
        .await
        .ok()?;
    let extra = match probe::extra_probe_command(host, &[]) {
        Some(command) => run_local_probe(host, "sh", &["-c", &command]).await.ok()?,
        None => String::new(),
    };

    Some(probe::snapshot(host, &failed, &restarting, &extra))
}

/// Runs one local probe command under the host's timeout, returning its stdout.
async fn run_local_probe(
    host: &HostEntity,
    program: &str,
    args: &[&str],
) -> Result<String, NodError> {
    use crate::infrastructure::health::probe;

    let probe_run = tokio::time::timeout(
        probe::timeout(host),
        Command::new(program).args(args).output(),
    )
    .await;

    match probe_run {
        Ok(Ok(output)) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Ok(Err(error)) => Err(NodError::health_check(format!(
            "cannot run the local health probe: {error}"
        ))),
        Err(_) => Err(NodError::health_check(format!(
            "health probe on {} timed out",
            host.name
        ))),
    }
}

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
    ) -> Result<DeploymentReport, NodError> {
        use crate::infrastructure::health::probe;
        let start = Instant::now();
        tracing::info!(host = %host.name, "Activating configuration...");

        let is_agentless = host.target_kind == crate::domain::host::TargetKind::Agentless
            || !closure.join("bin/switch-to-configuration").exists();

        let root = running_as_root();

        // Observe the host as it is, before this activation can change it.
        // Best-effort: an unreadable baseline is a statement of ignorance, not
        // a reason to skip verification (ADR-029).
        let baseline = snapshot_local(host).await;

        let watched = if is_agentless {
            let activate_bin = resolve_activation_binary(closure, &host.name)?;
            activation::watch(Command::new(&activate_bin).args([action])).await
        } else {
            // Register as new system generation for switch and boot (ADR-003, ADR-015).
            // Without updating the profile link, bootloaders (like systemd-boot) cannot see
            // the new generation and rebooting reverts to the old configuration.
            if matches!(action, "switch" | "boot") {
                let mut profile_cmd = privileged("nix-env", root);
                let profile_status = profile_cmd
                    .args([
                        "-p",
                        "/nix/var/nix/profiles/system",
                        "--set",
                        closure.to_str().unwrap(),
                    ])
                    .status()
                    .await;

                if profile_status.is_err() || !profile_status.unwrap().success() {
                    return Err(NodError::local_activate(
                        "failed to update /nix/var/nix/profiles/system generation",
                    ));
                }
            }

            let switch_bin = closure.join("bin/switch-to-configuration");
            activate(host, &switch_bin, action, root).await
        };

        let watched = match watched {
            Ok(watched) => watched,
            Err(_) => {
                return Err(NodError::local_activate(
                    "failed to launch activation executable",
                ))
            }
        };
        if !watched.status.success() {
            return Err(NodError::local_activate(
                "activation reported non-zero failure exit code",
            ));
        }

        let touched: Vec<String> = activation::touched_units(&watched.markers)
            .into_iter()
            .collect();

        let health = if !probe::enabled(host) {
            HealthVerdict::Unverified {
                reason: "health checks are disabled for this host".to_string(),
            }
        } else if reboot_required() {
            HealthVerdict::PendingReboot
        } else {
            match verify_local_health(host, baseline.as_ref(), &touched).await {
                Ok(verdict) => verdict,
                Err(error) => HealthVerdict::Unverified {
                    reason: error.to_string(),
                },
            }
        };

        if verbose {
            tracing::debug!(
                host = %host.name,
                elapsed = ?start.elapsed(),
                "Activation finished"
            );
        }
        Ok(DeploymentReport { health })
    }

    async fn rollback(&self, host: &HostEntity, _profile: &SshProfile) -> Result<(), NodError> {
        tracing::info!(
            host = %host.name,
            "Rolling back local host to previous generation..."
        );
        // Re-invoke the previous generation's switch directly. Asking
        // `nixos-rebuild --rollback` resolves `/etc/nixos` first and fails
        // before it rolls anything back (see `rollback::ROLLBACK_SCRIPT`).
        let status = Command::new("sh")
            .arg("-c")
            .arg(crate::infrastructure::deployment::rollback::ROLLBACK_SCRIPT)
            .status()
            .await;
        if status.is_err() {
            return Err(NodError::rollback_failure(
                "failed to launch the rollback script",
            ));
        }
        if !status.unwrap().success() {
            return Err(NodError::rollback_failure(
                "the rollback script reported failure",
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

/// Discovers the executable entrypoint inside an activation closure (ADR-026).
fn resolve_activation_binary(closure: &Path, name: &str) -> Result<PathBuf, NodError> {
    let bin_dir = closure.join("bin");

    // 1. Standard activate
    let activate = bin_dir.join("activate");
    if activate.exists() {
        return Ok(activate);
    }

    // 2. Standard switch-to-configuration
    let switch = bin_dir.join("switch-to-configuration");
    if switch.exists() {
        return Ok(switch);
    }

    // 3. Named binary: bin/<name>
    let by_name = bin_dir.join(name);
    if by_name.exists() {
        return Ok(by_name);
    }

    // 4. Prefixed binary: bin/activate-<name>
    let by_activate_name = bin_dir.join(format!("activate-{}", name));
    if by_activate_name.exists() {
        return Ok(by_activate_name);
    }

    // 5. If exactly one executable file exists in bin/
    if let Ok(entries) = std::fs::read_dir(&bin_dir) {
        let mut binaries = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                binaries.push(path);
            }
        }
        if binaries.len() == 1 {
            return Ok(binaries.pop().unwrap());
        }
    }

    // 6. Direct executable file
    if closure.is_file() {
        return Ok(closure.to_path_buf());
    }

    Err(NodError::local_activate(format!(
        "could not find activation executable in {} for target '{}'",
        closure.display(),
        name
    )))
}

use crate::domain::cache::{CachePushReport, StoreOptimizeReport};
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
        let is_root = running_as_root();
        let mut cmd = if options.dry_run {
            Command::new("nix-collect-garbage")
        } else {
            privileged("nix-collect-garbage", is_root)
        };

        if let Some(keep) = options.keep {
            if !options.dry_run {
                let mut env_cmd = privileged("nix-env", is_root);
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

    async fn optimize_store(
        &self,
        host: &HostEntity,
        _profile: &SshProfile,
    ) -> Result<StoreOptimizeReport, NodError> {
        let output = Command::new("nix-store").arg("--optimise").output().await;

        match output {
            Ok(out) if out.status.success() => Ok(StoreOptimizeReport {
                host_name: host.name.clone(),
                ok: true,
                freed_bytes: None,
                error: None,
            }),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Ok(StoreOptimizeReport {
                    host_name: host.name.clone(),
                    ok: false,
                    freed_bytes: None,
                    error: Some(stderr),
                })
            }
            Err(e) => Ok(StoreOptimizeReport {
                host_name: host.name.clone(),
                ok: false,
                freed_bytes: None,
                error: Some(format!("failed to launch nix-store --optimise: {e}")),
            }),
        }
    }

    async fn push_cache(
        &self,
        host: &HostEntity,
        _profile: &SshProfile,
        closure: &Path,
        cache_uri: &str,
    ) -> Result<CachePushReport, NodError> {
        let output = Command::new("nix")
            .args(["copy", "--to", cache_uri, &closure.to_string_lossy()])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => Ok(CachePushReport {
                host_name: host.name.clone(),
                closure_path: closure.to_path_buf(),
                cache_uri: cache_uri.to_string(),
                ok: true,
                error: None,
            }),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Ok(CachePushReport {
                    host_name: host.name.clone(),
                    closure_path: closure.to_path_buf(),
                    cache_uri: cache_uri.to_string(),
                    ok: false,
                    error: Some(stderr),
                })
            }
            Err(e) => Ok(CachePushReport {
                host_name: host.name.clone(),
                closure_path: closure.to_path_buf(),
                cache_uri: cache_uri.to_string(),
                ok: false,
                error: Some(format!("failed to launch nix copy: {e}")),
            }),
        }
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

    #[test]
    fn root_commands_do_not_go_through_sudo() {
        let direct = privileged("nix-env", true)
            .as_std()
            .get_program()
            .to_string_lossy()
            .to_string();
        assert_eq!(direct, "nix-env");

        let elevated = privileged("nix-env", false);
        let elevated = elevated.as_std();
        assert_eq!(elevated.get_program().to_string_lossy(), "sudo");
        assert_eq!(
            elevated
                .get_args()
                .next()
                .map(|a| a.to_string_lossy().to_string()),
            Some("nix-env".to_string())
        );
    }

    #[test]
    fn unit_names_are_sanitized_for_systemd() {
        assert_eq!(sanitize_unit_name("cld-ops-01"), "cld-ops-01");
        assert_eq!(sanitize_unit_name("host.example"), "host-example");
        assert_eq!(sanitize_unit_name("möbius"), "m-bius");
    }
}
