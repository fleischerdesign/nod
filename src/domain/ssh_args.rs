//! Shared `ssh(1)` argument construction from an [`SshProfile`].
//!
//! Dependency-free (domain layer) so both the `nod ssh`/`nod exec` command
//! paths and the SSH transport adapter consume the same argument knowledge:
//! connection flags are never re-derived in two places (ADR-007). `SshProfile`
//! is an already-resolved value object; this module only maps it to an argv.

use crate::domain::host::SshProfile;

/// Builds the connection-only `ssh(1)` flags for a profile: `-p <port>` for a
/// non-default port, `-i <identity>` for an identity file, `-J <proxy>` for a
/// proxy jump and any `extra_ssh_args` verbatim. No target or remote command
/// is appended. Used both to assemble the full `ssh` argv and as the
/// `NIX_SSHOPTS` value that `nix copy --to ssh://` forwards to its internal
/// ssh, so identity/proxy/extra-args are honoured on every transport path.
pub fn build_ssh_opts(profile: &SshProfile) -> Vec<String> {
    let mut opts = vec![
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        "ControlPath=/tmp/nod-ssh-%r@%h:%p".to_string(),
        "-o".to_string(),
        "ControlPersist=60s".to_string(),
    ];
    if profile.port() != 22 {
        opts.push("-p".to_string());
        opts.push(profile.port().to_string());
    }
    if let Some(identity) = profile.identity_file() {
        opts.push("-i".to_string());
        opts.push(identity.display().to_string());
    }
    if let Some(proxy) = profile.proxy_jump() {
        opts.push("-J".to_string());
        opts.push(proxy.to_string());
    }
    for extra in profile.extra_ssh_args() {
        opts.push(extra.clone());
    }
    opts
}

/// Builds the `ssh(1)` argument vector for one host profile.
///
/// Emits the connection flags (see [`build_ssh_opts`]), the `user@host`
/// target, an optional `sudo` prefix and the trailing remote command
/// verbatim. When `sudo` is set and the command is empty an interactive root
/// shell (`sudo -i`) is requested.
pub fn build_ssh_args(
    profile: &SshProfile,
    target_host: &str,
    sudo: bool,
    command: &[String],
) -> Vec<String> {
    let mut args = build_ssh_opts(profile);
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
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "root@atlas"
            ]
        );
    }

    #[test]
    fn custom_port_adds_dash_p() {
        let profile = profile_with("root", 2200, None, None, &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "-p",
                "2200",
                "root@atlas"
            ]
        );
    }

    #[test]
    fn identity_file_adds_dash_i() {
        let profile = profile_with("root", 22, Some("/path/key"), None, &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "-i",
                "/path/key",
                "root@atlas"
            ]
        );
    }

    #[test]
    fn proxy_jump_adds_dash_j() {
        let profile = profile_with("root", 22, None, Some("bastion"), &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "-J",
                "bastion",
                "root@atlas"
            ]
        );
    }

    #[test]
    fn port_identity_and_proxy_compose() {
        let profile = profile_with("philipp", 2200, Some("/tmp/id_rsa"), Some("bastion"), &[]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "-p",
                "2200",
                "-i",
                "/tmp/id_rsa",
                "-J",
                "bastion",
                "philipp@atlas"
            ]
        );
    }

    #[test]
    fn extra_ssh_args_are_preserved_positionally() {
        let profile = profile_with("root", 22, None, None, &["-o", "KeepAlive=1"]);
        let args = build_ssh_args(&profile, "atlas", false, &[]);
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "-o",
                "KeepAlive=1",
                "root@atlas"
            ]
        );
    }

    #[test]
    fn sudo_with_interactive_shell_runs_sudo_i() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(&profile, "atlas", true, &[]);
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "root@atlas",
                "sudo",
                "-i"
            ]
        );
    }

    #[test]
    fn sudo_with_trailing_command_prepends_sudo() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(
            &profile,
            "atlas",
            true,
            &["apt-get".to_string(), "update".to_string()],
        );
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "root@atlas",
                "sudo",
                "apt-get",
                "update"
            ]
        );
    }

    #[test]
    fn remote_command_is_append_without_sudo() {
        let profile = SshProfile::for_host(&HostEntity::new("atlas", "atlas", false));
        let args = build_ssh_args(
            &profile,
            "atlas",
            false,
            &["uname".to_string(), "-a".to_string()],
        );
        assert_eq!(
            args,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/tmp/nod-ssh-%r@%h:%p",
                "-o",
                "ControlPersist=60s",
                "root@atlas",
                "uname",
                "-a"
            ]
        );
    }
}
