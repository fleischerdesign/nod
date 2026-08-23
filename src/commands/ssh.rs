//! `nod ssh` command: open an interactive SSH session or execute a remote
//! command on a single resolved host (ADR-006 single-target invariant).
//!
//! Argument construction is a pure function ([`build_ssh_args`]) over an
//! `SshProfile`, so it is unit-testable without any ssh/Nix/network. The
//! connection itself is a plain `ssh` child process with inherited stdio so
//! interactive terminal sessions and remote commands behave like native
//! ssh(1).

use std::path::Path;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::domain::errors::NodError;
use crate::domain::host::SshProfile;

/// Builds the `ssh(1)` argument vector for one host profile.
///
/// Emits `-p <port>` for a non-default port, `-i <identity>` for an identity
/// file, `-J <proxy>` for a proxy jump, any `extra_ssh_args` verbatim, and
/// finally the `user@host` target. When `sudo` is set the remote command is
/// prefixed with `sudo`; an empty command with `--sudo` requests an
/// interactive root shell (`sudo -i`).
pub fn build_ssh_args(
    profile: &SshProfile,
    target_host: &str,
    sudo: bool,
    command: &[String],
) -> Vec<String> {
    let mut args = Vec::<String>::new();
    if profile.port() != 22 {
        args.push("-p".to_string());
        args.push(profile.port().to_string());
    }
    if let Some(identity) = profile.identity_file() {
        args.push("-i".to_string());
        args.push(identity.display().to_string());
    }
    if let Some(proxy) = profile.proxy_jump() {
        args.push("-J".to_string());
        args.push(proxy.to_string());
    }
    for extra in profile.extra_ssh_args() {
        args.push(extra.clone());
    }
    args.push(format!("{}@{}", profile.user(), target_host));
    if sudo {
        args.push("sudo".to_string());
        if command.is_empty() {
            args.push("-i".to_string());
        }
    }
    for part in command {
        args.push(part.clone());
    }
    args
}

/// Runs `nod ssh`: discovers hosts, resolves a single target via
/// [`TargetSelection::select_exact_one`], derives its `SshProfile` and opens
/// an ssh session (or runs a local shell/command for a directly addressed
/// local host) with inherited stdio.
pub async fn execute(
    ctx: AppContext,
    flake: Option<&Path>,
    target: Option<&str>,
    tag: Option<&str>,
    role: Option<&str>,
    sudo: bool,
    command: &[String],
) -> Result<(), NodError> {
    let flake_path = flake.unwrap_or_else(|| Path::new("."));
    let evaluator = ctx.evaluator();
    let hosts = evaluator.discover_hosts(flake_path, false).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let host = TargetSelection::select_exact_one(hosts, target, tag, role, false, &local_hostname)?;

    if host.is_local {
        return run_local(sudo, command).await;
    }

    let profile = SshProfile::for_host(&host);
    let args = build_ssh_args(&profile, &host.target_host, sudo, command);
    run_process("ssh", &args).await
}

/// Runs an external program, inheriting stdio from the invoking terminal.
/// A non-zero exit or a launch failure surfaces as a typed `NodError`.
async fn run_process(program: &str, args: &[String]) -> Result<(), NodError> {
    let mut process = Command::new(program);
    process.args(args);
    let status = process.status().await;
    if status.is_err() {
        return Err(NodError::deployment(format!("failed to launch `{program}`")));
    }
    if !status.unwrap().success() {
        return Err(NodError::deployment(format!("`{program}` reported failure")));
    }
    Ok(())
}

/// Runs a command (or opens an interactive shell) locally, for a directly
/// addressed local host.
async fn run_local(sudo: bool, command: &[String]) -> Result<(), NodError> {
    if command.is_empty() {
        if sudo {
            let args = vec!["-i".to_string()];
            return run_process("sudo", &args).await;
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        return run_process(&shell, &[]).await;
    }
    let mut exec = Vec::<String>::new();
    if sudo {
        exec.push("sudo".to_string());
    }
    for part in command {
        exec.push(part.clone());
    }
    let program = exec[0].clone();
    let mut args = Vec::<String>::new();
    for (index, part) in exec.iter().enumerate() {
        if index > 0 {
            args.push(part.clone());
        }
    }
    run_process(&program, &args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostEntity;

    /// Builds a remote-only profile with the requested connection settings.
    fn profile_with(
        user: &str,
        port: u16,
        identity: Option<&str>,
        proxy: Option<&str>,
        extra: &[&str],
    ) -> SshProfile {
        let mut profile = SshProfile::new(user, port);
        for arg in extra {
            profile = profile.with_extra_ssh_arg(arg.to_string());
        }
        if let Some(key) = identity {
            profile = profile.with_identity_file(std::path::PathBuf::from(key));
        }
        if let Some(hop) = proxy {
            profile = profile.with_proxy_jump(hop);
        }
        profile
    }

    #[test]
    fn default_root_at_host() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(args, ["root@atlas"]);
    }

    #[test]
    fn custom_port_adds_dash_p() {
        let profile = profile_with("root", 2200, None, None, &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(args, ["-p", "2200", "root@atlas"]);
    }

    #[test]
    fn identity_file_adds_dash_i() {
        let profile = profile_with("root", 22, Some("/path/key"), None, &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(args, ["-i", "/path/key", "root@atlas"]);
    }

    #[test]
    fn proxy_jump_adds_dash_j() {
        let profile = profile_with("root", 22, None, Some("bastion"), &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(args, ["-J", "bastion", "root@atlas"]);
    }

    #[test]
    fn port_identity_and_proxy_compose() {
        let profile = profile_with("philipp", 2200, Some("/tmp/id_rsa"), Some("bastion"), &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(
            args,
            ["-p", "2200", "-i", "/tmp/id_rsa", "-J", "bastion", "philipp@atlas"]
        );
    }

    #[test]
    fn extra_ssh_args_are_preserved_positionally() {
        let profile = profile_with("root", 22, None, None, &["-o", "KeepAlive=1"]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(args, ["-o", "KeepAlive=1", "root@atlas"]);
    }

    #[test]
    fn sudo_with_interactive_shell_runs_sudo_i() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(&profile, "atlas", true, &[]);
        assert_eq!(args, ["root@atlas", "sudo", "-i"]);
    }

    #[test]
    fn sudo_with_trailing_command_prepends_sudo() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(&profile, "atlas", true, &["apt-get".to_string(), "update".to_string()]);
        assert_eq!(args, ["root@atlas", "sudo", "apt-get", "update"]);
    }

    #[test]
    fn remote_command_is_append_without_sudo() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(&profile, "atlas", false, &["uname".to_string(), "-a".to_string()]);
        assert_eq!(args, ["root@atlas", "uname", "-a"]);
    }
}