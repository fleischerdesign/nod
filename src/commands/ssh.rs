//! `nod ssh` command: open an interactive SSH session or execute a remote
//! command on a single resolved host (ADR-006 single-target invariant).
//!
//! Argument construction is a pure function ([`build_ssh_args`]) over an
//! `SshProfile`, so it is unit-testable without any ssh/Nix/network. The
//! connection itself is a plain `ssh` child process with inherited stdio so
//! interactive terminal sessions and remote commands behave like native
//! ssh(1).

use std::path::Path;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::spawn::{run_inherited, split_program_args};
use crate::domain::errors::NodError;
use crate::domain::ssh_args::build_ssh_args;

/// Runs `nod ssh`: discovers hosts, resolves a single target via
/// [`TargetSelection::select_exact_one`], resolves its connection profile via
/// [`AppContext::resolved_profile`] and opens an ssh session (or runs a local
/// shell/command for a directly addressed local host) with inherited stdio.
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
    let hosts = evaluator.discover_hosts_strict(flake_path, false).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    // `ssh` is single-host: resolve the default set (the whole fleet when no
    // criteria are given — `DefaultScope::All` — so a bare `nod ssh` still
    // connects to a sole fleet host), then gate on exactly one.
    let resolved = resolve_targets(
        hosts,
        &local_hostname,
        target,
        tag,
        role,
        false,
        DefaultScope::All,
    );
    let host =
        TargetSelection::select_exact_one(resolved, None, None, None, false, &local_hostname)?;

    if host.is_local {
        return run_local(sudo, command).await;
    }

    let profile = ctx.resolved_profile(&host).await?;
    let args = build_ssh_args(&profile, &host.target_host, sudo, command);
    run_process("ssh", &args).await
}

/// Runs an external program, inheriting stdio from the invoking terminal.
/// A non-zero exit or a launch failure surfaces as a typed `NodError`.
/// Delegates the spawn + exit-status mapping to the shared transport helper
/// ([`run_inherited`]) with the `nod ssh` error contract.
async fn run_process(program: &str, args: &[String]) -> Result<(), NodError> {
    run_inherited(
        program,
        args,
        || NodError::deployment(format!("failed to launch `{program}`")),
        || NodError::deployment(format!("`{program}` reported failure")),
    )
    .await
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
    exec.extend(command.iter().cloned());
    let (program, args) =
        split_program_args(&exec).expect("exec is non-empty: sudo-suffixed or a non-empty command");
    run_process(&program, &args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ssh_runs_argv0_directly_not_through_a_shell() {
        // `nod ssh <local> -- <cmd> args` runs <cmd> directly: argv[0] is the
        // program and the rest are its arguments. This diverges deliberately
        // from the exec-fleet transport, which builds a single `sh -c` line
        // (see `exec_fleet::build_local_args`) so shell pipelines/globs work
        // on the remote side. The shared [`split_program_args`] keeps the
        // split rule in one place while the two contracts stay distinct.
        let exec = vec!["uname".to_string(), "-a".to_string()];
        let (program, args) = split_program_args(&exec).unwrap();
        assert_eq!(program, "uname");
        assert_eq!(args, ["-a"]);
        assert_ne!(program, "sh");
    }
}
