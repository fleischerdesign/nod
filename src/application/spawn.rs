//! Shared child-process spawn helpers for `nod ssh` and the exec-fleet
//! transport (DRY). Both commands spawn an external program with an argv and
//! must classify a launch failure / non-zero exit, but under different
//! contracts:
//!
//! * Interactive runs (`nod ssh`, interactive local shell) inherit stdio from
//!   the terminal and surface failures as typed [`NodError`]s — see
//!   [`run_inherited`].
//! * Fleet exec captures stdout/stderr/exit-status into a per-host result and
//!   never aborts the run because of a single bad host — see [`run_captured`].
//!
//! The two local-execution contracts intentionally diverge: `nod ssh` runs the
//! command's argv[0] *directly* as the program (no shell), whereas exec builds
//! an `sh -c` line so shell pipelines/globs work on the remote side. That
//! divergence is preserved — this module only shares the spawn/argv machinery,
//! not the shelling policy.

use crate::domain::errors::NodError;
use tokio::process::Command;

/// Splits `["prog", "a", "b"]` into `("prog", ["a", "b"])`: argv[0] is the
/// program and everything after it is an argument. Shared by the `nod ssh`
/// and exec local runners so the split rule lives in one place.
pub fn split_program_args(argv: &[String]) -> (String, Vec<String>) {
    let program = argv[0].clone();
    let args = argv[1..].to_vec();
    (program, args)
}

/// Runs `program` with `args`, inheriting stdio from the invoking terminal so
/// interactive sessions behave like a natively spawned binary. A launch
/// failure maps to `map_launch`; a non-zero exit maps to `map_exit`. This is
/// the `nod ssh` contract — failures are typed errors, not captured output.
pub async fn run_inherited(
    program: &str,
    args: &[String],
    map_launch: impl FnOnce() -> NodError,
    map_exit: impl FnOnce() -> NodError,
) -> Result<(), NodError> {
    let mut process = Command::new(program);
    process.args(args);
    match process.status().await {
        Err(_) => Err(map_launch()),
        Ok(status) if !status.success() => Err(map_exit()),
        Ok(_) => Ok(()),
    }
}

/// The per-host outcome of one exec-fleet process run: the numeric exit code,
/// whether it succeeded, and the captured stdout/stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// Numeric exit status, or `1` when the process could not be launched.
    pub exit_code: i32,
    /// True when the process exited 0 and was spawned successfully.
    pub success: bool,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// Runs `program` with `args`, capturing stdout/stderr and the numeric exit
/// status (exec contract). It never returns a typed error — a launch failure
/// is embedded as `exit_code` 1 with an explanatory stderr message so a bad
/// host degrades the per-host result instead of aborting the fleet run.
pub async fn run_captured(program: &str, args: &[String], host_name: &str) -> Captured {
    let mut process = Command::new(program);
    process.args(args);
    let output = process.output().await;
    match output {
        Ok(output) => Captured {
            exit_code: output.status.code().unwrap_or(1),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(_) => Captured {
            exit_code: 1,
            success: false,
            stdout: String::new(),
            stderr: format!("failed to launch `{program}` for {host_name}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_program_args_keeps_first_word_as_program() {
        let argv = vec!["uname".to_string(), "-a".to_string(), "-r".to_string()];
        let (program, args) = split_program_args(&argv);
        assert_eq!(program, "uname");
        assert_eq!(args, ["-a", "-r"]);
    }

    #[test]
    fn split_program_args_handles_a_bare_program() {
        let argv = vec!["uptime".to_string()];
        let (program, args) = split_program_args(&argv);
        assert_eq!(program, "uptime");
        assert!(args.is_empty());
    }

    #[tokio::test]
    async fn run_inherited_maps_launch_failure_to_map_launch() {
        let err = run_inherited(
            "nod-no-such-binary-xyz",
            &[],
            || NodError::deployment("failed to launch `xyz`".to_string()),
            || NodError::deployment("`xyz` reported failure".to_string()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, NodError::Deployment { .. }));
        assert!(err.to_string().contains("failed to launch"));
    }

    #[tokio::test]
    async fn run_inherited_maps_non_zero_exit_to_map_exit() {
        let args = vec!["-c".to_string(), "exit 7".to_string()];
        let err = run_inherited(
            "sh",
            &args,
            || NodError::deployment("launch".to_string()),
            || NodError::deployment("`sh` reported failure".to_string()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, NodError::Deployment { .. }));
        assert!(err.to_string().contains("reported failure"));
    }

    #[tokio::test]
    async fn run_inherited_success_is_ok() {
        let args = vec!["-c".to_string(), "true".to_string()];
        assert!(
            run_inherited("sh", &args, || unreachable!(), || unreachable!())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn run_captured_returns_success_and_stdout() {
        let args = vec!["-c".to_string(), "printf hello".to_string()];
        let captured = run_captured("sh", &args, "web-01").await;
        assert!(captured.success);
        assert_eq!(captured.exit_code, 0);
        assert_eq!(captured.stdout, "hello");
    }

    #[tokio::test]
    async fn run_captured_reports_the_numeric_exit_code() {
        let args = vec!["-c".to_string(), "exit 9".to_string()];
        let captured = run_captured("sh", &args, "web-01").await;
        assert!(!captured.success);
        assert_eq!(captured.exit_code, 9);
    }

    #[tokio::test]
    async fn run_captured_embeds_launch_failure_instead_of_returning_err() {
        let captured = run_captured("nod-no-such-binary-xyz", &[], "web-01").await;
        assert!(!captured.success);
        assert_eq!(captured.exit_code, 1);
        assert!(captured.stderr.contains("web-01"));
        assert!(captured.stderr.contains("failed to launch"));
    }
}
