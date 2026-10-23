//! SSH CLI deployer adapter: `nix copy --to` + `ssh ... switch` for remote
//! hosts. A dumb transport: it consumes only the *passed-in* resolved
//! `SshProfile` and never re-derives a connection profile itself (ADR-007).

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::time::Instant;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::health::{DeploymentReport, HealthSnapshot, HealthVerdict};
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::plan::DeploymentAction;
use crate::domain::ports::deployer::DeployerPort;
use crate::domain::ssh_args::{build_ssh_args, build_ssh_opts};
use crate::infrastructure::deployment::activation;

/// Deploys remote hosts through store copy over SSH and a remote switch
/// activation.
pub struct SshCliDeployer;

/// `true` when an SSH failure looks like a credential or host-key problem.
///
/// SSH reports authentication failures only as text, so the classification
/// lives in one place and turns into a typed error at the boundary. Everything
/// else is an outage: routed, silent, or refused.
fn looks_like_auth_failure(stderr: &str) -> bool {
    let lowered = stderr.to_lowercase();
    [
        "permission denied",
        "host key verification failed",
        "no matching host key",
        "too many authentication failures",
        "authentication failed",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}

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
        let port = host.nod_config.ssh.port.unwrap_or(host.target_port);
        let timeout_secs = host.nod_config.ssh.connect_timeout_secs.unwrap_or(3) as u64;

        let addr = (host.target_host.as_str(), port);
        match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        {
            Ok(Ok(_stream)) => Ok(true),
            _ => Err(NodError::unreachable(host.name.clone())),
        }
    }

    async fn current_closure(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
    ) -> Result<Option<PathBuf>, NodError> {
        // Resolve the remote symlink over the same transport as activation;
        // a non-zero exit means the host has no live closure yet (or the
        // query failed), both of which surface as "no active closure".
        let args = build_ssh_args(
            profile,
            &host.target_host,
            false,
            &["readlink /run/current-system".to_string()],
        );
        let output = run_ssh("ssh", &args, || {
            NodError::deployment(format!(
                "failed to query current closure of {} over SSH",
                host.name
            ))
        })
        .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // A failed query is not "no live closure": conflating the two made
            // a status reader without the deploy key report every remote host
            // as drifted. The caller must be able to tell "unknown" from
            // "absent".
            //
            // SSH reports authentication failure as an exit status and a
            // message; that is the only signal it offers, so it is matched in
            // one place and surfaced as a typed error.
            if looks_like_auth_failure(&stderr) {
                return Err(NodError::auth(format!(
                    "{} refused our credentials: {}",
                    host.name,
                    stderr.trim()
                )));
            }
            return Err(NodError::deployment(format!(
                "cannot read the live closure of {}: {}",
                host.name,
                stderr.trim()
            )));
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
        profile: &SshProfile,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<DeploymentReport, NodError> {
        use crate::infrastructure::health::probe;
        let start = Instant::now();

        // AC3: validate the action against the deployment allow-list and quote
        // the path before any SSH run — an unknown action is a config error at
        // the command boundary (defense-in-depth; the CLI already constrains it).
        let switch_bin = closure.join("bin/switch-to-configuration");
        let remote_cmd = build_remote_command(&switch_bin, action, closure)?;

        tracing::info!(
            target_host = %host.target_host,
            "Copying closure over SSH..."
        );

        // `nix copy --to ssh://` runs its own ssh; the connection-only flags
        // (identity/proxy/extra args) are forwarded via NIX_SSHOPTS so they are
        // honoured on the store transfer too (AC3); the store URI carries the
        // port only when it differs from the default (ADR-007).
        let mut store_target = format!("ssh://{}@{}", profile.user(), host.target_host);
        if profile.port() != 22 {
            store_target = format!("{}:{}", store_target, profile.port());
        }
        let ssh_opts = build_ssh_opts(profile);
        let mut copy = Command::new("nix");
        copy.args(["copy", "--to", &store_target, closure.to_str().unwrap()]);
        if !ssh_opts.is_empty() {
            copy.env("NIX_SSHOPTS", ssh_opts.join(" "));
        }
        let copy_status = copy.status().await;

        if copy_status.is_err() {
            return Err(NodError::store_transfer(format!(
                "failed to launch `nix copy` for {}",
                host.name
            )));
        }
        if !copy_status.unwrap().success() {
            return Err(NodError::store_transfer(host.name.clone()));
        }

        // Observe the host before this activation can change it; an unreadable
        // baseline is ignorance, not evidence of health (ADR-029).
        let baseline = snapshot_remote(host, profile).await;

        tracing::info!(
            target_host = %host.target_host,
            "Activating remote configuration..."
        );

        let args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);
        let mut ssh_command = Command::new("ssh");
        ssh_command.args(&args);
        let watched = activation::watch(&mut ssh_command).await.map_err(|_| {
            NodError::remote_activate(format!("failed to launch `ssh` for {}", host.name))
        })?;
        if !watched.status.success() {
            return Err(NodError::remote_activate(host.name.clone()));
        }
        let touched: Vec<String> = activation::touched_units(&watched.markers)
            .into_iter()
            .collect();

        let health = if !probe::enabled(host) {
            HealthVerdict::Unverified {
                reason: "health checks are disabled for this host".to_string(),
            }
        } else {
            match run_remote_probe(host, profile, probe::REBOOT_REQUIRED_COMMAND).await {
                Ok(output) if probe::reboot_required(&output) => HealthVerdict::PendingReboot,
                _ => match verify_remote_health(host, profile, baseline.as_ref(), &touched).await {
                    Ok(verdict) => verdict,
                    Err(error) => HealthVerdict::Unverified {
                        reason: error.to_string(),
                    },
                },
            }
        };

        if verbose {
            tracing::debug!(
                host = %host.name,
                elapsed = ?start.elapsed(),
                "Remote deployment finished"
            );
        }

        Ok(DeploymentReport { health })
    }

    async fn observe_health(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
    ) -> Result<HealthVerdict, NodError> {
        use crate::infrastructure::health::probe;

        if !probe::enabled(host) {
            return Ok(HealthVerdict::Unverified {
                reason: "health checks are disabled for this host".to_string(),
            });
        }
        verify_remote_health(host, profile, None, &[]).await
    }

    async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError> {
        tracing::info!(
            host = %host.name,
            "Rolling back remote host to previous generation..."
        );

        // Remote generation query: list the prior profile links available.
        let query_args = build_ssh_args(
            profile,
            &host.target_host,
            false,
            &["/nix/var/nix/profiles".to_string()],
        );
        let query = run_ssh_inherited("ssh", &query_args, || {
            NodError::rollback_failure(format!(
                "failed to query remote generations for {}",
                host.name
            ))
        })
        .await?;
        if !query.success() {
            return Err(NodError::rollback_failure(format!(
                "remote generation query failed for {}",
                host.name
            )));
        }

        // Roll back by switching the previous generation directly. A flake
        // reference here would make the rollback depend on `/etc/nixos`
        // existing, which is exactly what fails when it is needed.
        let remote_cmd = crate::infrastructure::deployment::rollback::ROLLBACK_SCRIPT.to_string();
        let rollback_args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);
        let rollback_status = run_ssh_inherited("ssh", &rollback_args, || {
            NodError::rollback_failure(format!("failed to launch ssh rollback for {}", host.name))
        })
        .await?;
        if !rollback_status.success() {
            return Err(NodError::rollback_failure(host.name.clone()));
        }
        Ok(())
    }

    async fn reboot(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError> {
        tracing::info!(host = %host.name, "Initiating remote system reboot over SSH...");
        let remote_cmd = "sudo systemctl reboot || sudo reboot".to_string();
        let ssh_args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);

        // Spawning reboot over SSH often exits with status 255 because sshd terminates the socket on shutdown
        let _ = Command::new("ssh").args(&ssh_args).status().await;
        Ok(())
    }
}

use crate::domain::cache::{CachePushReport, StoreOptimizeReport};
use crate::domain::generation::{CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration};
use crate::domain::ports::store::StorePort;
use crate::infrastructure::deployment::local_deployer::parse_system_profiles_stat_output;

#[async_trait]
impl StorePort for SshCliDeployer {
    async fn list_generations(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
    ) -> Result<Vec<SystemGeneration>, NodError> {
        let remote_cmd =
            "stat -c '%n %Y %N' /nix/var/nix/profiles/system* 2>/dev/null || true".to_string();
        let ssh_args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);

        let output = run_ssh("ssh", &ssh_args, || {
            NodError::health_check(format!(
                "failed to query generations over SSH for {}",
                host.name
            ))
        })
        .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_system_profiles_stat_output(&stdout))
    }

    async fn collect_garbage(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
        options: &GcOptions,
    ) -> Result<GcReport, NodError> {
        let dry_flag = if options.dry_run { " --dry-run" } else { "" };
        let remote_cmd = if let Some(keep) = options.keep {
            format!("sudo nix-env -p /nix/var/nix/profiles/system --delete-generations +{keep} && sudo nix-collect-garbage{dry_flag}")
        } else if let Some(ref older_than) = options.older_than {
            format!("sudo nix-collect-garbage --delete-older-than {older_than}{dry_flag}")
        } else {
            format!("sudo nix-collect-garbage -d{dry_flag}")
        };

        let ssh_args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);
        let output = run_ssh("ssh", &ssh_args, || {
            NodError::deployment(format!(
                "failed to execute remote garbage collection on {}",
                host.name
            ))
        })
        .await?;

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
        profile: &SshProfile,
        closure: &Path,
        options: &CopyOptions,
    ) -> Result<CopyReport, NodError> {
        let mut cmd = Command::new("nix");
        cmd.arg("copy");
        if let Some(ref to) = options.to {
            cmd.args(["--to", to]);
        } else {
            let target_uri = format!("ssh://{}@{}", profile.user(), host.target_host);
            cmd.args(["--to", &target_uri]);
        }
        if let Some(ref from) = options.from {
            cmd.args(["--from", from]);
        }
        cmd.arg(closure.to_str().unwrap_or(""));

        let output = cmd.output().await.map_err(|e| {
            NodError::deployment(format!("failed to execute nix copy over SSH: {e}"))
        })?;

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
        profile: &SshProfile,
    ) -> Result<StoreOptimizeReport, NodError> {
        let optimize_cmd = "sudo nix-store --optimise || nix-store --optimise".to_string();
        let ssh_args = build_ssh_args(profile, &host.target_host, false, &[optimize_cmd]);
        let output = Command::new("ssh").args(&ssh_args).output().await;

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
                error: Some(format!("failed to run remote nix-store --optimise: {e}")),
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
                error: Some(format!("failed to launch nix copy to cache: {e}")),
            }),
        }
    }
}

/// Spawns `program` with `args`, capturing stdout/stderr into buffers and
/// waiting for the process to exit. Returns `Ok(output)` once the process has
/// launched and finished; a launch failure is mapped to `map_launch`. The
/// caller inspects `output.status` (and stdout) for exit handling.
///
/// This is used only by [`current_closure`], which needs the captured stdout
/// to read the resolved closure path and treats a non-zero exit as "no active
/// closure". The console-facing activation and rollback invocations use
/// [`run_ssh_inherited`] instead, so their remote logs are streamed to the
/// operator's terminal rather than being captured into never-read buffers.
///
/// This centralizes the process-spawn + launch-failure mapping that was
/// previously hand-rolled at each call site (ADR-007 keeps the argv
/// construction in [`build_ssh_args`]; this is the transport half).
async fn run_ssh(
    program: &str,
    args: &[String],
    map_launch: impl FnOnce() -> NodError,
) -> Result<Output, NodError> {
    match Command::new(program).args(args).output().await {
        Ok(output) => Ok(output),
        Err(_) => Err(map_launch()),
    }
}

/// Verifies the freshly activated remote system over the same transport.
async fn verify_remote_health(
    host: &HostEntity,
    profile: &SshProfile,
    baseline: Option<&HealthSnapshot>,
    touched: &[String],
) -> Result<HealthVerdict, NodError> {
    use crate::domain::health::verdict_against;
    use crate::infrastructure::health::probe;

    let failed = run_remote_probe(host, profile, probe::FAILED_UNITS_COMMAND).await?;
    let restarting = run_remote_probe(host, profile, probe::RESTARTING_UNITS_COMMAND).await?;
    let extra = match probe::extra_probe_command(host, touched) {
        Some(command) => run_remote_probe(host, profile, &command).await?,
        None => String::new(),
    };
    let current = probe::snapshot(host, &failed, &restarting, &extra);

    Ok(verdict_against(
        baseline,
        &current,
        probe::max_restarts(host),
    ))
}

/// Observes the remote host's health inputs, tolerating probe failures.
async fn snapshot_remote(host: &HostEntity, profile: &SshProfile) -> Option<HealthSnapshot> {
    use crate::infrastructure::health::probe;

    if !probe::enabled(host) {
        return Some(HealthSnapshot::clean());
    }

    let failed = run_remote_probe(host, profile, probe::FAILED_UNITS_COMMAND)
        .await
        .ok()?;
    let restarting = run_remote_probe(host, profile, probe::RESTARTING_UNITS_COMMAND)
        .await
        .ok()?;
    let extra = match probe::extra_probe_command(host, &[]) {
        Some(command) => run_remote_probe(host, profile, &command).await.ok()?,
        None => String::new(),
    };

    Some(probe::snapshot(host, &failed, &restarting, &extra))
}

/// Runs one probe command on `host` over SSH, returning its stdout.
///
/// The script is piped to `sh -s`: the remote login shell may be fish (this
/// fleet's is), and a POSIX probe script must not be parsed by it. `sh -s`
/// reads the script from stdin, so no quoting layer is involved either.
async fn run_remote_probe(
    host: &HostEntity,
    profile: &SshProfile,
    command: &str,
) -> Result<String, NodError> {
    use crate::infrastructure::health::probe;
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let args = crate::domain::ssh_args::build_ssh_probe_args(profile, &host.target_host, "sh -s");
    let probe_run = tokio::time::timeout(probe::timeout(host), async {
        let mut child = Command::new("ssh")
            .args(&args)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(command.as_bytes()).await?;
        }
        child.wait_with_output().await
    })
    .await;

    match probe_run {
        Ok(Ok(output)) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let reason = stderr.trim();
                return Err(NodError::health_check(format!(
                    "health probe on {} failed: {}",
                    host.name,
                    if reason.is_empty() {
                        "no stderr"
                    } else {
                        reason
                    }
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(Err(error)) => Err(NodError::health_check(format!(
            "failed to run the health probe for {}: {error}",
            host.name
        ))),
        Err(_) => Err(NodError::health_check(format!(
            "health probe on {} timed out",
            host.name
        ))),
    }
}

/// Spawns `program` with `args` with stdio inherited (streamed to the
/// operator's terminal) and waits for it to exit, returning the `ExitStatus`.
/// A launch failure is mapped to `map_launch`. This mirrors [`run_ssh`] but
/// uses `.status()` so the remote activation and `nixos-rebuild --rollback`
/// logs (including non-critical stderr warnings that still exit 0) reach the
/// terminal instead of being silently dropped. It exactly restores the
/// pre-refactor `.status()` behaviour at the console-facing SSH sites. The
/// caller maps a non-zero exit to the per-site `NodError` variant.
async fn run_ssh_inherited(
    program: &str,
    args: &[String],
    map_launch: impl FnOnce() -> NodError,
) -> Result<ExitStatus, NodError> {
    match Command::new(program).args(args).status().await {
        Ok(status) => Ok(status),
        Err(_) => Err(map_launch()),
    }
}

/// Single-quotes a string for the remote `/bin/sh -c` shell (AC3). A literal
/// `'` is escaped with the standard POSIX idiom (`'\''`), so paths containing
/// spaces or quotes survive as a single word on the remote shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Validates the activation action against the known deployment allow-list
/// and builds the remote shell command (AC3). For `switch` and `boot`, it
/// updates the `/nix/var/nix/profiles/system` generation via `nix-env` so
/// bootloaders (e.g. systemd-boot) register the new generation.
/// The paths are single-quoted so they survive the remote shell; the allow-listed
/// action is a single word carrying no shell metacharacters. An unknown action is a
/// `NodError::config` at the command boundary.
fn build_remote_command(
    switch_bin: &Path,
    action: &str,
    closure: &Path,
) -> Result<String, NodError> {
    if DeploymentAction::parse(action).is_none() {
        return Err(NodError::config(format!(
            "unknown deployment action '{action}'; expected one of: switch, boot, test, dry-run, build"
        )));
    }
    let switch_invocation = format!(
        "{} {}",
        shell_quote(&switch_bin.display().to_string()),
        action
    );
    if matches!(action, "switch" | "boot") {
        let set_profile_cmd = format!(
            "sudo nix-env -p /nix/var/nix/profiles/system --set {}",
            shell_quote(&closure.display().to_string())
        );
        Ok(format!("{set_profile_cmd} && {switch_invocation}"))
    } else {
        Ok(switch_invocation)
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

    #[test]
    fn unknown_action_is_rejected_as_a_config_error() {
        let err = build_remote_command(
            Path::new("/nix/store/abc-system/bin/switch-to-configuration"),
            "evil; rm -rf /",
            Path::new("/nix/store/abc-system"),
        )
        .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[test]
    fn path_with_spaces_is_quoted_in_remote_command() {
        let cmd = build_remote_command(
            Path::new("/nix/store/my system/bin/switch-to-configuration"),
            "switch",
            Path::new("/nix/store/my system"),
        )
        .unwrap();
        assert_eq!(
            cmd,
            "sudo nix-env -p /nix/var/nix/profiles/system --set '/nix/store/my system' && '/nix/store/my system/bin/switch-to-configuration' switch"
        );
    }

    #[test]
    fn test_action_does_not_set_system_profile() {
        let cmd = build_remote_command(
            Path::new("/nix/store/my system/bin/switch-to-configuration"),
            "test",
            Path::new("/nix/store/my system"),
        )
        .unwrap();
        assert_eq!(
            cmd,
            "'/nix/store/my system/bin/switch-to-configuration' test"
        );
    }

    #[test]
    fn path_with_single_quote_is_escaped() {
        let cmd = build_remote_command(
            Path::new("/nix/store/it's here/system/bin/switch-to-configuration"),
            "switch",
            Path::new("/nix/store/it's here/system"),
        )
        .unwrap();
        assert!(cmd.starts_with("sudo nix-env -p /nix/var/nix/profiles/system --set '"));
        assert!(cmd.contains("'\\''"));
    }

    #[test]
    fn all_known_deployment_actions_are_accepted() {
        for action in ["switch", "boot", "test", "dry-run", "build"] {
            assert!(
                build_remote_command(
                    Path::new("/nix/store/abc-system/bin/switch-to-configuration"),
                    action,
                    Path::new("/nix/store/abc-system"),
                )
                .is_ok(),
                "action={}",
                action
            );
        }
    }

    #[tokio::test]
    async fn run_ssh_maps_launch_failure_to_the_provided_error() {
        // A spawn failure (missing binary) surfaces the injected constructor,
        // preserving the per-site NodError variant and message.
        let args = vec!["-x".to_string()];
        let err = run_ssh("nod-no-such-binary-xyz", &args, || {
            NodError::rollback_failure("failed to query remote generations for atlas".to_string())
        })
        .await
        .unwrap_err();
        assert!(matches!(err, NodError::Deployment { .. }));
        assert!(err.to_string().contains("rollback failed"));
        assert!(err
            .to_string()
            .contains("failed to query remote generations for atlas"));
    }

    #[tokio::test]
    async fn run_ssh_surfaces_non_zero_exit_as_non_success_output() {
        // A non-zero exit is *not* misclassified as a launch failure: it comes
        // back as `Ok` output whose status the caller inspects to pick the
        // right typed error (or, for `current_closure`, `Ok(None)`).
        let args = vec!["-c".to_string(), "exit 3".to_string()];
        let output = run_ssh("sh", &args, || NodError::deployment("boom".to_string()))
            .await
            .expect("spawn should succeed");
        assert!(!output.status.success());
    }

    #[tokio::test]
    async fn run_ssh_captures_stdout_on_success() {
        let args = vec!["-c".to_string(), "printf hello".to_string()];
        let output = run_ssh("sh", &args, || NodError::deployment("boom".to_string()))
            .await
            .expect("spawn should succeed");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
    }

    #[tokio::test]
    async fn run_ssh_inherited_maps_launch_failure_to_the_provided_error() {
        // The inherited-stdio helper exposes the same launch-failure seam as
        // `run_ssh`: a spawn failure (missing binary) surfaces the injected
        // constructor, preserving the per-site NodError variant and message.
        let args = vec!["-x".to_string()];
        let err = run_ssh_inherited("nod-no-such-binary-xyz", &args, || {
            NodError::remote_activate("failed to launch `ssh` for atlas".to_string())
        })
        .await
        .unwrap_err();
        assert!(matches!(err, NodError::Deployment { .. }));
        assert!(err
            .to_string()
            .contains("failed to execute remote activation over SSH"));
        assert!(err.to_string().contains("failed to launch `ssh` for atlas"));
    }

    #[tokio::test]
    async fn run_ssh_inherited_surfaces_non_zero_exit_as_non_success_status() {
        // A non-zero exit is *not* misclassified as a launch failure: it comes
        // back as `Ok` exit status whose `success()` the caller inspects to
        // pick the right typed error.
        let args = vec!["-c".to_string(), "exit 3".to_string()];
        let status = run_ssh_inherited("sh", &args, || NodError::deployment("boom".to_string()))
            .await
            .expect("spawn should succeed");
        assert!(!status.success());
    }

    #[tokio::test]
    async fn deploy_and_activate_rejects_unknown_action_before_any_ssh() {
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let profile = SshProfile::for_host(&host);
        let closure = Path::new("/nix/store/abc-system");
        let result = SshCliDeployer::new()
            .deploy_and_activate(&host, &profile, closure, "evil", false)
            .await;
        assert!(matches!(result, Err(NodError::Config { .. })));
    }

    #[tokio::test]
    async fn check_reachability_succeeds_on_open_tcp_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let mut host = HostEntity::new("local-test", "127.0.0.1", false);
        host.target_port = port;

        let deployer = SshCliDeployer::new();
        let reachable = deployer.check_reachability(&host).await;
        assert!(reachable.is_ok());
        assert!(reachable.unwrap());
    }

    #[tokio::test]
    async fn check_reachability_fails_on_closed_port() {
        let mut host = HostEntity::new("local-test", "127.0.0.1", false);
        host.target_port = 1; // Unlikely to be listening
        host.nod_config.ssh.connect_timeout_secs = Some(1);

        let deployer = SshCliDeployer::new();
        let reachable = deployer.check_reachability(&host).await;
        assert!(reachable.is_err());
        assert!(matches!(
            reachable.unwrap_err(),
            NodError::HealthCheck { .. }
        ));
    }
}
