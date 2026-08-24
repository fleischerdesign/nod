//! `ExecFleetUseCase`: run a shell command across a resolved fleet, bounded
//! by the ADR-005 concurrency policy.
//!
//! Target resolution is the caller's concern (ADR-006 `TargetSelection`); the
//! use case receives the resolved `HostEntity` list. Each host derives its
//! `SshProfile` and executes the command remotely over `ssh` — or locally via
//! `sh -c` when `host.is_local` — capturing the numeric exit code, stdout,
//! stderr and wall-clock duration into an `ExecResult`. A
//! `tokio::sync::Semaphore` bounds in-flight hosts to `--concurrency N`;
//! `--fail-fast` clears a shared abort flag so hosts that have not yet
//! started report a skipped `ExecResult` instead of running.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::ssh_args::build_ssh_args;

/// Exit code reported for a host that never started because `--fail-fast`
/// aborted the run after an earlier host failed.
pub const EXEC_SKIPPED_EXIT_CODE: i32 = -1;

/// The per-host result of a remote command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    /// The host this result was gathered from.
    pub host_name: String,
    /// The numeric exit status of the child process (ssh or `sh -c`), or
    /// [`EXEC_SKIPPED_EXIT_CODE`] when `--fail-fast` skipped the host before
    /// it started.
    pub exit_code: i32,
    /// Captured stdout of the remote/local command.
    pub stdout: String,
    /// Captured stderr of the remote/local command.
    pub stderr: String,
    /// Wall-clock time spent on this host's execution, in milliseconds.
    pub duration_ms: u64,
    /// True when the command exited with status 0 and was not skipped.
    pub success: bool,
}

impl ExecResult {
    /// Builds a result from an observed execution.
    pub fn new(
        host_name: impl Into<String>,
        exit_code: i32,
        stdout: String,
        stderr: String,
        duration_ms: u64,
        success: bool,
    ) -> Self {
        Self {
            host_name: host_name.into(),
            exit_code,
            stdout,
            stderr,
            duration_ms,
            success,
        }
    }

    /// Builds the "never started" result for a host skipped by
    /// `--fail-fast` (ADR-005 error-recovery mode).
    pub fn skipped(host_name: impl Into<String>) -> Self {
        Self {
            host_name: host_name.into(),
            exit_code: EXEC_SKIPPED_EXIT_CODE,
            stdout: String::new(),
            stderr: "aborted by --fail-fast".to_string(),
            duration_ms: 0,
            success: false,
        }
    }

    /// Returns `true` when `--fail-fast` skipped this host before it ran.
    pub fn is_skipped(&self) -> bool {
        self.exit_code == EXEC_SKIPPED_EXIT_CODE
    }
}

/// Fleet remote-command policy executor.
pub struct ExecFleetUseCase {
    ctx: Arc<AppContext>,
}

impl ExecFleetUseCase {
    /// Builds the use case over a seeded context, from which connection
    /// profiles are resolved (ADR-007).
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Runs `command` across `hosts`, at most `concurrency` at once.
    ///
    /// Every host is spawned immediately but each must acquire a semaphore
    /// permit before executing (ADR-005). When `fail_fast` is set, the first
    /// failing host flips the shared abort flag; hosts that acquire a permit
    /// afterwards return a skipped `ExecResult` and never touch the transport.
    pub async fn execute(
        &self,
        hosts: Vec<HostEntity>,
        command: Vec<String>,
        concurrency: usize,
        sudo: bool,
        fail_fast: bool,
    ) -> Result<Vec<ExecResult>, NodError> {
        if hosts.is_empty() {
            return Err(NodError::config("no hosts targeted for remote execution"));
        }
        if command.is_empty() {
            return Err(NodError::config(
                "no command supplied; pass the command after `--`",
            ));
        }
        if concurrency == 0 {
            return Err(NodError::config("--concurrency must be at least 1"));
        }

        let sem = Arc::new(Semaphore::new(concurrency));
        let abort = Arc::new(AtomicBool::new(false));
        let mut set = JoinSet::<ExecResult>::new();

        for host in hosts {
            let sem_c = sem.clone();
            let abort_c = abort.clone();
            let command_c = command.clone();
            let ctx_c = self.ctx.clone();
            set.spawn(async move {
                run_one(ctx_c, host, command_c, sudo, fail_fast, sem_c, abort_c).await
            });
        }

        Ok(set.join_all().await)
    }
}

/// Executes the command for one host under a semaphore permit (ADR-005),
/// honouring the shared `--fail-fast` abort flag.
async fn run_one(
    ctx: Arc<AppContext>,
    host: HostEntity,
    command: Vec<String>,
    sudo: bool,
    fail_fast: bool,
    sem: Arc<Semaphore>,
    abort: Arc<AtomicBool>,
) -> ExecResult {
    let acquired = sem.acquire().await;
    if acquired.is_err() {
        return ExecResult::new(
            host.name.clone(),
            1,
            String::new(),
            "failed to acquire the concurrency permit".to_string(),
            0,
            false,
        );
    }
    // The permit's `Drop` frees a slot for the next queued host; keep it
    // alive across the whole execution by binding it to a named variable.
    let _permit = acquired.unwrap();

    if fail_fast && abort.load(Ordering::Relaxed) {
        return ExecResult::skipped(host.name.clone());
    }

    let start = Instant::now();
    let result = if host.is_local {
        run_local(&host, &command, sudo, start).await
    } else {
        let profile = match ctx.resolved_profile(&host).await {
            Ok(profile) => profile,
            Err(err) => {
                return ExecResult::new(
                    host.name.clone(),
                    1,
                    String::new(),
                    format!("failed to resolve connection profile: {err}"),
                    elapsed_ms(start),
                    false,
                )
            }
        };
        let args = build_ssh_args(&profile, &host.target_host, sudo, &command);
        run_process(&host, "ssh", &args, start).await
    };

    if fail_fast && !result.success {
        abort.store(true, Ordering::Relaxed);
    }
    result
}

/// Builds the local `sh -c` invocation for `command`, prefixing `sudo` when
/// requested. Pure and testable without running anything.
fn build_local_args(command: &[String], sudo: bool) -> Vec<String> {
    let mut line = String::new();
    let mut first = true;
    for part in command {
        if first {
            line = part.clone();
            first = false;
        } else {
            line = format!("{} {}", line, part);
        }
    }
    if sudo {
        line = format!("sudo {}", line);
    }
    vec!["sh".to_string(), "-c".to_string(), line]
}

/// Runs the local invocation: `sh -c <command>` (with `sudo` inside the
/// shell line when requested), keeping the program/args split for the
/// process spawn.
async fn run_local(
    host: &HostEntity,
    command: &[String],
    sudo: bool,
    start: Instant,
) -> ExecResult {
    let exec = build_local_args(command, sudo);
    let program = exec[0].clone();
    let mut args = Vec::<String>::new();
    for (index, part) in exec.iter().enumerate() {
        if index > 0 {
            args.push(part.clone());
        }
    }
    run_process(host, &program, &args, start).await
}

/// Spawns `program` with `args`, capturing stdout/stderr and the numeric
/// exit status into an [`ExecResult`]. A spawn failure surfaces as a
/// per-host failure with a launch message (`docs/spec/exec-command.spec.md`).
async fn run_process(
    host: &HostEntity,
    program: &str,
    args: &[String],
    start: Instant,
) -> ExecResult {
    let mut process = Command::new(program);
    process.args(args);
    let output = process.output().await;
    let duration_ms = elapsed_ms(start);
    match output {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(1);
            let success = output.status.success();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            ExecResult::new(
                host.name.clone(),
                exit_code,
                stdout,
                stderr,
                duration_ms,
                success,
            )
        }
        Err(_) => ExecResult::new(
            host.name.clone(),
            1,
            String::new(),
            format!("failed to launch `{program}` for {}", host.name),
            duration_ms,
            false,
        ),
    }
}

/// Converts the elapsed time into whole milliseconds.
fn elapsed_ms(start: Instant) -> u64 {
    let elapsed = start.elapsed();
    elapsed.as_secs() * 1000 + elapsed.subsec_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::context::AppContext;
    use std::sync::Arc;

    fn host(name: &str, is_local: bool) -> HostEntity {
        HostEntity::new(name, name, is_local)
    }

    /// A minimal context (no config store, so `resolved_profile` falls back to
    /// the primitive profile). Exec runs never touch the evaluator; the local
    /// hosts below never touch the SSH transport.
    fn ctx() -> Arc<AppContext> {
        Arc::new(AppContext::new(
            Arc::new(crate::infrastructure::nix::cli_evaluator::NixCliEvaluator::new()),
            Arc::new(crate::infrastructure::deployment::local_deployer::LocalDeployer::new()),
            Arc::new(crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer::new()),
        ))
    }

    #[test]
    fn skipped_results_carry_the_documented_sentinel() {
        let result = ExecResult::skipped("web-01");
        assert!(result.is_skipped());
        assert_eq!(result.exit_code, EXEC_SKIPPED_EXIT_CODE);
        assert!(!result.success);
        assert!(result.stderr.contains("--fail-fast"));
    }

    #[test]
    fn build_local_args_joins_command_without_sudo() {
        let args = build_local_args(&["uname".to_string(), "-a".to_string()], false);
        assert_eq!(args, ["sh", "-c", "uname -a"]);
    }

    #[test]
    fn build_local_args_prefixes_sudo() {
        let args = build_local_args(&["uname".to_string(), "-a".to_string()], true);
        assert_eq!(args, ["sh", "-c", "sudo uname -a"]);
    }

    #[test]
    fn build_local_args_preserves_a_single_word_command() {
        let args = build_local_args(&["uptime".to_string()], false);
        assert_eq!(args, ["sh", "-c", "uptime"]);
    }

    #[tokio::test]
    async fn local_host_executes_and_reports_success() {
        let hosts = vec![host("jello", true)];
        let results = ExecFleetUseCase::new(ctx())
            .execute(
                hosts,
                vec!["echo".to_string(), "hi".to_string()],
                4,
                false,
                false,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].host_name, "jello");
        assert!(results[0].success);
        assert_eq!(results[0].exit_code, 0);
        assert!(results[0].stdout.contains("hi"));
        assert!(results[0].stderr.is_empty());
    }

    #[tokio::test]
    async fn failing_command_reports_the_numeric_exit_code() {
        let hosts = vec![host("atlas", true)];
        let results = ExecFleetUseCase::new(ctx())
            .execute(
                hosts,
                vec!["exit".to_string(), "3".to_string()],
                4,
                false,
                false,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert_eq!(results[0].exit_code, 3);
    }

    #[tokio::test]
    async fn empty_fleet_is_a_config_error() {
        let err = ExecFleetUseCase::new(ctx())
            .execute(vec![], vec!["true".to_string()], 4, false, false)
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[tokio::test]
    async fn empty_command_is_a_config_error() {
        let err = ExecFleetUseCase::new(ctx())
            .execute(vec![host("jello", true)], vec![], 4, false, false)
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[tokio::test]
    async fn zero_concurrency_is_a_config_error() {
        let err = ExecFleetUseCase::new(ctx())
            .execute(
                vec![host("jello", true)],
                vec!["true".to_string()],
                0,
                false,
                false,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[tokio::test]
    async fn concurrency_bounds_in_flight_hosts() {
        let hosts = vec![
            host("h1", true),
            host("h2", true),
            host("h3", true),
            host("h4", true),
            host("h5", true),
            host("h6", true),
        ];
        let command = vec!["sleep".to_string(), "0.2".to_string()];
        let start = Instant::now();
        let results = ExecFleetUseCase::new(ctx())
            .execute(hosts, command, 2, false, false)
            .await
            .unwrap();
        let elapsed = elapsed_ms(start);

        assert_eq!(results.len(), 6);
        assert!(results.iter().all(|r| r.success));
        // 6 x 200ms with 2 workers runs in ~3 waves (~600ms total); a serial
        // run would take ~1200ms. The 1s bar proves the semaphore bounded
        // in-flight hosts well below serial.
        assert!(
            elapsed < 1000,
            "expected bounded concurrency, took {}ms",
            elapsed
        );
    }

    #[tokio::test]
    async fn fail_fast_skips_hosts_that_have_not_started() {
        let hosts = vec![
            host("bad", true),
            host("a", true),
            host("b", true),
            host("c", true),
        ];
        let results = ExecFleetUseCase::new(ctx())
            .execute(
                hosts,
                vec!["exit".to_string(), "2".to_string()],
                1,
                false,
                true,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 4);
        assert!(!results[0].success);
        assert_eq!(results[0].exit_code, 2);
        for skipped in &results[1..] {
            assert!(
                skipped.is_skipped(),
                "{} should have been skipped",
                skipped.host_name
            );
            assert_eq!(skipped.exit_code, EXEC_SKIPPED_EXIT_CODE);
        }
    }

    #[tokio::test]
    async fn without_fail_fast_every_host_still_runs() {
        let hosts = vec![host("h1", true), host("h2", true), host("h3", true)];
        let results = ExecFleetUseCase::new(ctx())
            .execute(
                hosts,
                vec!["exit".to_string(), "1".to_string()],
                1,
                false,
                false,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        for result in &results {
            // Every host ran the failing command; none was skipped.
            assert!(
                !result.is_skipped(),
                "{} must not be skipped",
                result.host_name
            );
            assert_eq!(result.exit_code, 1);
            assert!(!result.success);
        }
    }
}
