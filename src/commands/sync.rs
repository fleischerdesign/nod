//! `nod sync` command: one reconciliation cycle (ADR-028).
//!
//! Selection stays in presentation (ADR-006): the command resolves the
//! requested scope and hands a lazy selector to the use case, which only calls
//! it after the cycle decided that something must change.

use std::path::Path;
use std::sync::Arc;

use colored::Colorize;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::sync_reconciler::SyncReconcilerUseCase;
use crate::commands::wiring;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::sync::{SyncHostResult, SyncReport};
use crate::domain::watch::SyncOptions;

/// Executes exactly one cycle and maps its outcome to an exit status.
pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    options: SyncOptions,
) -> Result<(), NodError> {
    validate(&options)?;
    let ctx = wiring::with_sync(ctx, &options.state_dir)?;
    let _lock = wiring::sync_lock(&options.state_dir)?;
    let ctx = Arc::new(ctx);

    let report = cycle(&ctx, flake_path, &options).await?;
    emit(&report, &options);
    record_audit(&ctx, &report).await;

    if let Some(reason) = &report.refused {
        return Err(NodError::refused(reason.clone()));
    }
    // Unreachable or skipped hosts are reported, not fatal: a laptop that is
    // off must not make `nod sync --once` fail. Only a host that actually
    // failed - or a cycle-level error - is an unsuccessful run.
    let failed = report
        .hosts
        .iter()
        .filter(|host| host.result.is_failure())
        .count();
    if failed > 0 || report.error.is_some() {
        let detail = report
            .error
            .clone()
            .unwrap_or_else(|| format!("{failed} host(s) failed"));
        return Err(NodError::deployment(format!(
            "{detail} at revision {}",
            report.revision
        )));
    }

    Ok(())
}

/// Rejects option combinations that cannot mean anything useful.
pub(crate) fn validate(options: &SyncOptions) -> Result<(), NodError> {
    if options.interval_secs < 15 {
        return Err(NodError::config(
            "--interval must be at least 15 seconds (ADR-028)",
        ));
    }
    if options.jitter_percent > 50 {
        return Err(NodError::config("--jitter must be at most 50 percent"));
    }
    if options.concurrency == 0 {
        return Err(NodError::config("--concurrency must be at least 1"));
    }
    Ok(())
}

/// Runs one cycle over the lazily selected host set.
pub(crate) async fn cycle(
    ctx: &Arc<AppContext>,
    flake_path: &Path,
    options: &SyncOptions,
) -> Result<SyncReport, NodError> {
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let flake = flake_path.to_path_buf();
    let selection_ctx = ctx.clone();
    let selection_options = options.clone();

    SyncReconcilerUseCase::new(ctx.clone())
        .reconcile_once(flake_path, options, || {
            let sel_ctx = selection_ctx.clone();
            let sel_options = selection_options.clone();
            let sel_flake = flake.clone();
            let hostname = local_hostname.clone();
            async move {
                let hosts = sel_ctx
                    .evaluator()
                    .discover_hosts_degraded(&sel_flake, sel_options.verbose)
                    .await?;
                select_hosts(hosts, &sel_options, &hostname)
            }
        })
        .await
}

/// Resolves the requested scope into a host list.
///
/// There is no implicit scope: `--all`, at least one target, or tag/role
/// filters must name the fleet explicitly (ADR-028).
pub fn select_hosts(
    hosts: Vec<HostEntity>,
    options: &SyncOptions,
    local_hostname: &str,
) -> Result<Vec<HostEntity>, NodError> {
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |host: HostEntity| {
        if seen.insert(host.name.clone()) {
            selected.push(host);
        }
    };

    if options.all {
        for host in hosts {
            push(host);
        }
    } else if options.targets.is_empty() {
        return Err(NodError::config(
            "no target scope: pass --target/--all or set [sync].targets",
        ));
    } else {
        for target in &options.targets {
            let matched = TargetSelection::select(
                hosts.clone(),
                Some(target.as_str()),
                None,
                None,
                target == "all",
                local_hostname,
            );
            if matched.is_empty() {
                return Err(NodError::config(format!(
                    "no host matches target '{target}'"
                )));
            }
            for host in matched {
                push(host);
            }
        }
    }

    if let Some(tag) = &options.tag {
        selected.retain(|host| host.has_tag(tag));
    }
    if let Some(role) = &options.role {
        selected.retain(|host| host.matches_role(role));
    }

    if selected.is_empty() {
        return Err(NodError::config("no hosts matched the selection"));
    }

    Ok(selected)
}

/// Renders the report as human-readable text or JSON.
pub(crate) fn emit(report: &SyncReport, options: &SyncOptions) {
    if options.json {
        match serde_json::to_string_pretty(report) {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("cannot serialize sync report: {e}"),
        }
        return;
    }

    if let Some(reason) = &report.refused {
        println!("{} sync refused: {}", "✗".red().bold(), reason);
        return;
    }

    let revision = report.revision.get(..12).unwrap_or(&report.revision);
    println!("{} revision {}", "→".cyan(), revision);

    if let Some(error) = &report.error {
        println!("  {} {}", "✗".red(), error);
    }

    for host in &report.hosts {
        let label = match host.result {
            SyncHostResult::Deployed => "deployed".green(),
            SyncHostResult::Unchanged => "unchanged".dimmed(),
            SyncHostResult::RolledBack => "rolled back".yellow(),
            SyncHostResult::Failed => "failed".red(),
            SyncHostResult::Unreachable => "unreachable".red(),
            SyncHostResult::Skipped => "skipped".yellow(),
        };
        println!("  {} {}", host.host.bold(), label);
        if let Some(detail) = &host.detail {
            println!("      {}", detail.dimmed());
        }
    }

    if report.hosts.is_empty() && report.error.is_none() {
        println!("  {} fleet already at this revision", "✓".green());
    }
    if report.backoff_secs > 0 {
        println!("  next attempt in {}s", report.backoff_secs);
    }
}

/// Appends one audit entry per host outcome.
pub(crate) async fn record_audit(ctx: &AppContext, report: &SyncReport) {
    let Ok(store) = ctx.audit_store() else {
        return;
    };
    for host in &report.hosts {
        let outcome = match host.result {
            SyncHostResult::Deployed => "deployed",
            SyncHostResult::Unchanged => "unchanged",
            SyncHostResult::RolledBack => "rolled_back",
            SyncHostResult::Failed => "failed",
            SyncHostResult::Unreachable => "unreachable",
            SyncHostResult::Skipped => "skipped",
        };
        if let Err(e) = store.record(&host.host, outcome).await {
            tracing::warn!("failed to record audit entry for {}: {e}", host.host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> SyncOptions {
        SyncOptions {
            interval_secs: 60,
            ..Default::default()
        }
    }

    #[test]
    fn a_missing_scope_is_a_configuration_error() {
        let hosts = vec![HostEntity::new("atlas", "10.0.0.8", false)];
        let error = select_hosts(hosts, &options(), "atlas").unwrap_err();
        assert!(
            error.to_string().contains("no target scope"),
            "got: {error}"
        );
    }

    #[test]
    fn several_targets_are_unioned_without_duplicates() {
        let hosts = vec![
            HostEntity::new("atlas", "10.0.0.8", false),
            HostEntity::new("boreas", "10.0.0.9", false),
        ];
        let mut options = options();
        options.targets = vec!["atlas".into(), "a*".into()];
        let selected = select_hosts(hosts, &options, "atlas").unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "atlas");
    }

    #[test]
    fn an_unknown_target_is_named_in_the_error() {
        let hosts = vec![HostEntity::new("atlas", "10.0.0.8", false)];
        let mut options = options();
        options.targets = vec!["missing".into()];
        let error = select_hosts(hosts, &options, "atlas").unwrap_err();
        assert!(error.to_string().contains("missing"), "got: {error}");
    }

    #[test]
    fn options_are_validated_before_any_work() {
        let mut too_fast = options();
        too_fast.interval_secs = 5;
        assert!(validate(&too_fast).is_err());

        let mut too_jittery = options();
        too_jittery.jitter_percent = 80;
        assert!(validate(&too_jittery).is_err());
    }
}
