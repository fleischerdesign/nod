//! `nod exec` command: run an arbitrary shell command across a resolved
//! fleet (ADR-006 target grammar, ADR-005 concurrency policy), rendering
//! per-host prefixed output or a JSON result array (`--json`), and failing
//! the command when any host failed.

use colored::Colorize;
use serde::Serialize;
use std::path::Path;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::exec_fleet::{ExecFleetUseCase, ExecResult};
use crate::domain::errors::NodError;

/// One row of the `--json` result array (`docs/spec/exec-command.spec.md`).
#[derive(Debug, Serialize)]
struct ExecRow {
    host: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
    duration_ms: u64,
}

impl ExecRow {
    fn from_result(result: &ExecResult) -> Self {
        Self {
            host: result.host_name.clone(),
            exit_code: result.exit_code,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            duration_ms: result.duration_ms,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ctx: AppContext,
    flake: Option<&Path>,
    target: Option<&str>,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    sudo: bool,
    concurrency: Option<usize>,
    fail_fast: bool,
    json: bool,
    command: &[String],
) -> Result<(), NodError> {
    let flake_path = flake.unwrap_or_else(|| Path::new("."));
    let concurrency = concurrency.unwrap_or(4);

    let evaluator = ctx.evaluator();
    let hosts = evaluator.discover_hosts(flake_path, false).await?;

    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let (effective_target, effective_all) =
        if !all && target.is_none() && tag.is_none() && role.is_none() {
            (Some("local"), false)
        } else {
            (target, all)
        };
    let targets = TargetSelection::select(
        hosts,
        effective_target,
        tag,
        role,
        effective_all,
        &local_hostname,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            effective_target.unwrap_or("all"),
            tag,
            role,
        ));
    }

    let results: Vec<ExecResult> =
        ExecFleetUseCase::execute(targets, command.to_vec(), concurrency, sudo, fail_fast)
            .await?;

    if json {
        let rows: Vec<ExecRow> = results.iter().map(ExecRow::from_result).collect();
        println!("{}", serde_json::to_string(&rows).unwrap());
    } else {
        render_text(&results, command);
    }

    let failed: Vec<&ExecResult> = results
        .iter()
        .filter(|r| !r.success && !r.is_skipped())
        .collect();
    let skipped = results.iter().filter(|r| r.is_skipped()).count();
    if failed.is_empty() && skipped == 0 {
        return Ok(());
    }
    if failed.is_empty() {
        return Err(NodError::deployment(format!(
            "run aborted by --fail-fast: {} of {} hosts were skipped",
            skipped,
            results.len()
        )));
    }
    let names: Vec<String> = failed.iter().map(|r| r.host_name.clone()).collect();
    let mut message = format!(
        "{} of {} hosts failed: [{}]",
        failed.len(),
        results.len(),
        names.join(", ")
    );
    if skipped > 0 {
        message = format!("{} ({} skipped)", message, skipped);
    }
    Err(NodError::deployment(message))
}

/// Renders the fleet result with per-host prefixes (`[host-name] output...`).
fn render_text(results: &Vec<ExecResult>, command: &[String]) {
    let cmd_line = command.join(" ");
    for result in results {
        let prefix = format!("[{}]", result.host_name);
        if result.is_skipped() {
            println!("{} {}", prefix.bold(), "aborted by --fail-fast".yellow());
            continue;
        }
        println!("{} $ {}", prefix.bold(), cmd_line);
        for line in result.stdout.lines() {
            println!("{} {}", prefix.bold(), line);
        }
        for line in result.stderr.lines() {
            println!("{} {}", prefix.bold().red(), line);
        }
        let status = format!("exit {} in {}ms", result.exit_code, result.duration_ms);
        let colored = if result.success {
            status.green()
        } else {
            status.red()
        };
        println!("{} {}", prefix.bold(), colored);
    }
    let succeeded = results.iter().filter(|r| r.success).count();
    let skipped = results.iter().filter(|r| r.is_skipped()).count();
    let failed = results.len() - succeeded - skipped;
    println!(
        "\n  {}",
        format!("{} succeeded, {} failed, {} skipped", succeeded, failed, skipped).dimmed()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(host_name: &str, exit_code: i32, stdout: &str, stderr: &str) -> ExecResult {
        ExecResult::new(
            host_name.to_string(),
            exit_code,
            stdout.to_string(),
            stderr.to_string(),
            12,
            exit_code == 0,
        )
    }

    #[test]
    fn json_row_carries_the_documented_fields() {
        let a = result("web-01", 0, "up", "");
        let b = result("db-01", 3, "", "boom");
        let rows: Vec<ExecRow> = vec![&a, &b].into_iter().map(ExecRow::from_result).collect();
        let json = serde_json::to_string(&rows).unwrap();
        assert!(json.contains("\"host\":\"web-01\""));
        assert!(json.contains("\"exit_code\":0"));
        assert!(json.contains("\"stdout\":\"up\""));
        assert!(json.contains("\"host\":\"db-01\""));
        assert!(json.contains("\"exit_code\":3"));
        assert!(json.contains("\"stderr\":\"boom\""));
        assert!(json.contains("\"duration_ms\":12"));
    }
}