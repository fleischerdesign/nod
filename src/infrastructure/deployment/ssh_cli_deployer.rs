//! SSH CLI deployer adapter: `nix copy --to` + `ssh ... switch` for remote
//! hosts. A dumb transport: it consumes only the *passed-in* resolved
//! `SshProfile` and never re-derives a connection profile itself (ADR-007).

use async_trait::async_trait;
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::plan::DeploymentAction;
use crate::domain::ports::deployer::DeployerPort;
use crate::domain::ssh_args::{build_ssh_args, build_ssh_opts};

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
        let output = Command::new("ssh").args(&args).output().await;
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
        profile: &SshProfile,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError> {
        let start = Instant::now();

        // AC3: validate the action against the deployment allow-list and quote
        // the path before any SSH run — an unknown action is a config error at
        // the command boundary (defense-in-depth; the CLI already constrains it).
        let switch_bin = closure.join("bin/switch-to-configuration");
        let remote_cmd = build_remote_command(&switch_bin, action)?;

        println!(
            "  {}",
            format!("Copying closure to {} over SSH...", host.target_host).dimmed()
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

        println!(
            "  {}",
            format!("Activating remote configuration on {}...", host.target_host).dimmed()
        );

        let args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);
        let ssh_status = Command::new("ssh").args(&args).status().await;

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

    async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError> {
        println!(
            "  {}",
            format!(
                "Rolling back remote host {} to previous generation...",
                host.name
            )
            .yellow()
        );

        // Remote generation query: list the prior profile links available.
        let query_args = build_ssh_args(
            profile,
            &host.target_host,
            false,
            &["/nix/var/nix/profiles".to_string()],
        );
        let query = Command::new("ssh").args(&query_args).status().await;
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
        let rollback_args = build_ssh_args(profile, &host.target_host, false, &[remote_cmd]);
        let rollback_status = Command::new("ssh").args(&rollback_args).status().await;
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

/// Single-quotes a string for the remote `/bin/sh -c` shell (AC3). A literal
/// `'` is escaped with the standard POSIX idiom (`'\''`), so paths containing
/// spaces or quotes survive as a single word on the remote shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Validates the activation action against the known deployment allow-list
/// and builds the remote shell command (AC3). The `switch_bin` path is
/// single-quoted so it survives the remote shell; the allow-listed action is
/// a single word carrying no shell metacharacters. An unknown action is a
/// `NodError::config` at the command boundary.
fn build_remote_command(switch_bin: &Path, action: &str) -> Result<String, NodError> {
    if DeploymentAction::parse(action).is_none() {
        return Err(NodError::config(format!(
            "unknown deployment action '{action}'; expected one of: switch, boot, test, dry-run, build"
        )));
    }
    Ok(format!(
        "{} {}",
        shell_quote(&switch_bin.display().to_string()),
        action
    ))
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
        )
        .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[test]
    fn path_with_spaces_is_quoted_in_remote_command() {
        let cmd = build_remote_command(
            Path::new("/nix/store/my system/bin/switch-to-configuration"),
            "switch",
        )
        .unwrap();
        assert_eq!(
            cmd,
            "'/nix/store/my system/bin/switch-to-configuration' switch"
        );
    }

    #[test]
    fn path_with_single_quote_is_escaped() {
        let cmd = build_remote_command(Path::new("/nix/store/it's here/system"), "switch").unwrap();
        assert!(cmd.starts_with("'"));
        assert!(cmd.contains("'\\''"));
    }

    #[test]
    fn all_known_deployment_actions_are_accepted() {
        for action in ["switch", "boot", "test", "dry-run", "build"] {
            assert!(
                build_remote_command(Path::new("/nix/store/abc-system"), action).is_ok(),
                "action={}",
                action
            );
        }
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
}
