use colored::Colorize;
use std::io;
use std::path::Path;
use std::process::ExitStatus;
use tokio::process::Command;

use crate::domain::errors::NodError;

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

    pub async fn run_all(flake_path: &Path) -> Result<(), NodError> {
        println!("{}", "> Quality gates".bold().cyan());

        let target_file = Self::resolve_target_file(flake_path);

        if target_file.exists() {
            let fmt_status = Command::new("nixfmt")
                .args(["--check", target_file.to_str().unwrap()])
                .status()
                .await;
            warn_gate("nixfmt", &fmt_status);
        }

        let deadnix_status = Command::new("deadnix")
            .args(["--fail", &flake_path.display().to_string()])
            .status()
            .await;
        deadnix_gate(&deadnix_status)?;

        let statix_status = Command::new("statix")
            .args(["check", &flake_path.display().to_string()])
            .status()
            .await;
        warn_gate("statix", &statix_status);

        println!("  {}", "✓ Passed".bold().green());
        Ok(())
    }
}

/// Classification of a single quality-gate spawn result (AC1).
enum GateClassification {
    /// Command launched and exited 0.
    Pass,
    /// Command could not be launched (missing binary / spawn error).
    LaunchError,
    /// Command launched but exited non-zero.
    FailedCheck,
}

/// Pure classification of a gate `status`, shared by the warn-only and hard
/// paths (AC1).
fn classify(status: &Result<ExitStatus, io::Error>) -> GateClassification {
    match status {
        Err(_) => GateClassification::LaunchError,
        Ok(s) if !s.success() => GateClassification::FailedCheck,
        Ok(_) => GateClassification::Pass,
    }
}

/// Builds the warning text for a warn-only gate outcome, or `None` when the
/// gate passed. Launch failures surface an explicit "not found; skipping"
/// warning (AC1) rather than silent success.
fn gate_warning(tool: &str, kind: &GateClassification) -> Option<String> {
    match kind {
        GateClassification::Pass => None,
        GateClassification::LaunchError => {
            Some(format!("⚠ {tool} not found; skipping {tool} check"))
        }
        GateClassification::FailedCheck => {
            let note = match tool {
                "nixfmt" => "reported formatting issues.",
                "statix" => "reported anti-patterns.",
                _ => "reported issues.",
            };
            Some(format!("⚠ {tool} {note}"))
        }
    }
}

/// Runs a warn-only gate (nixfmt, statix): a non-zero exit or a launch
/// failure stays a visible warning and never fails the run (AC1).
fn warn_gate(tool: &str, status: &Result<ExitStatus, io::Error>) {
    if let Some(msg) = gate_warning(tool, &classify(status)) {
        println!("  {}", msg.yellow());
    }
}

/// Runs the hard gate (deadnix): a launch failure or a non-zero exit is a
/// hard evaluation error (AC1). deadnix reports dead code, i.e. a check
/// outcome, so it is classified as an evaluation failure rather than a
/// malformed configuration.
fn deadnix_gate(status: &Result<ExitStatus, io::Error>) -> Result<(), NodError> {
    match classify(status) {
        GateClassification::Pass => Ok(()),
        GateClassification::LaunchError => Err(NodError::evaluation(
            "deadnix check could not run: failed to launch `deadnix`".to_string(),
        )),
        GateClassification::FailedCheck => Err(NodError::evaluation(
            "deadnix check failed (dead code detected).",
        )),
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

    fn launch_error() -> Result<ExitStatus, io::Error> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "No such file or directory",
        ))
    }

    fn failed_status() -> Result<ExitStatus, io::Error> {
        Ok(std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 1")
            .status()
            .unwrap())
    }

    fn ok_status() -> Result<ExitStatus, io::Error> {
        Ok(ExitStatus::default())
    }

    #[test]
    fn deadnix_launch_failure_is_a_hard_evaluation_error() {
        let err = deadnix_gate(&launch_error()).unwrap_err();
        assert!(matches!(err, NodError::Evaluation { .. }));
        assert!(err.to_string().contains("deadnix"));
    }

    #[test]
    fn deadnix_nonzero_exit_is_a_hard_evaluation_error() {
        let err = deadnix_gate(&failed_status()).unwrap_err();
        assert!(matches!(err, NodError::Evaluation { .. }));
    }

    #[test]
    fn deadnix_pass_is_ok() {
        assert!(deadnix_gate(&ok_status()).is_ok());
    }

    #[test]
    fn warn_gate_launch_failure_is_not_a_hard_error() {
        // nixfmt/statix degrade to a visible warning, never a hard failure.
        assert!(gate_warning("nixfmt", &classify(&launch_error())).is_some());
        warn_gate("statix", &launch_error());
    }

    #[test]
    fn warn_gate_missing_tool_warning_is_explicit() {
        let msg = gate_warning("nixfmt", &classify(&launch_error())).unwrap();
        assert!(msg.contains("nixfmt not found"));
        assert!(msg.contains("skipping nixfmt check"));
    }

    #[test]
    fn warn_gate_nonzero_exit_stays_warn_only() {
        let msg = gate_warning("statix", &classify(&failed_status())).unwrap();
        assert!(msg.contains("statix"));
        warn_gate("nixfmt", &failed_status());
    }
}
