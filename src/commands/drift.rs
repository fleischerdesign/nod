//! `nod drift` command: compare live host closures against the flake (ADR-003
//! observability). Backed by `DetectDriftUseCase` and rendered text/JSON.

use colored::Colorize;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::detect_drift::{DetectDriftUseCase, DriftReport};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

/// One JSON row of the drift report (`--json` shape).
#[derive(Debug, Serialize)]
struct DriftRow {
    host: String,
    active: Option<String>,
    flake: Option<String>,
    drifted: bool,
    error: Option<String>,
}

impl DriftRow {
    fn ok(host: &str, report: &DriftReport) -> Self {
        Self {
            host: host.to_string(),
            active: report.active_closure.as_ref().map(|p| p.display().to_string()),
            flake: report.flake_closure.as_ref().map(|p| p.display().to_string()),
            drifted: report.drifted,
            error: None,
        }
    }

    fn failed(host: &str, err: &NodError) -> Self {
        Self {
            host: host.to_string(),
            active: None,
            flake: None,
            drifted: true,
            error: Some(err.to_string()),
        }
    }
}

pub async fn execute(
    flake_path: &Path,
    verbose: bool,
    target: Option<&str>,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    json: bool,
) -> Result<(), NodError> {
    let config_store = TomlConfigStore::new(flake_path, CliOverrides::default())?;
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_config_store(Arc::new(config_store));
    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let (effective_target, effective_all) = if !all && target.is_none() && tag.is_none() && role.is_none() {
        (Some("local"), false)
    } else {
        (target, all)
    };
    let targets = TargetSelection::select(hosts, effective_target, tag, role, effective_all, &local_hostname);

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(effective_target.unwrap_or("all"), tag, role));
    }

    let use_case = DetectDriftUseCase::new(Arc::new(ctx));
    let mut rows = Vec::<DriftRow>::with_capacity(targets.len());
    for host in targets {
        let host: HostEntity = store.apply_to(host).await?;
        // A probe/build failure on one host must not abort the whole drift
        // sweep; it is surfaced per host below.
        let report = use_case.execute(&host, flake_path, verbose).await;
        match report {
            Ok(report) => rows.push(DriftRow::ok(&host.name, &report)),
            Err(err) => rows.push(DriftRow::failed(&host.name, &err)),
        }
    }

    if json {
        println!("{}", serde_json::to_string(&rows).unwrap());
    } else {
        render_text(&rows);
    }
    Ok(())
}

/// Renders the drift report as one row per host.
fn render_text(rows: &Vec<DriftRow>) {
    println!(
        "\n{:<20} {:<10} {:<42} {:<42}",
        "HOST".bold(),
        "DRIFT".bold(),
        "ACTIVE".bold(),
        "FLAKE".bold()
    );
    for row in rows {
        if let Some(err) = &row.error {
            println!(
                "{:<20} {:<10} {}",
                row.host,
                "check-failed".red(),
                err
            );
            continue;
        }
        let flag = if row.drifted {
            "drifted".yellow()
        } else {
            "in-sync".green()
        };
        println!(
            "{:<20} {:<10} {:<42} {:<42}",
            row.host,
            flag,
            row.active.as_deref().unwrap_or("(unknown)"),
            row.flake.as_deref().unwrap_or("-")
        );
    }
}