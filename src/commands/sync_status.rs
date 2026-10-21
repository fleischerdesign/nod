//! `nod sync-status` command: read-only view of the reconciler (ADR-028).

use std::path::Path;
use std::sync::Arc;

use colored::Colorize;

use crate::application::context::AppContext;
use crate::application::use_cases::sync_status::SyncStatusUseCase;
use crate::commands::wiring;
use crate::domain::errors::NodError;
use crate::domain::sync::{now_secs, SyncStatus};
use crate::domain::watch::SyncOptions;

/// Executes the status query and renders it.
pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    options: SyncOptions,
    hosts: Option<Vec<crate::domain::host::HostEntity>>,
) -> Result<(), NodError> {
    let ctx = wiring::with_sync(ctx, &options.state_dir)?;
    let status = SyncStatusUseCase::new(Arc::new(ctx))
        .execute(flake_path, &options, hosts)
        .await?;
    render(&status, options.json);
    Ok(())
}

/// Renders a duration since `since` in the largest useful unit.
fn age(now: u64, since: Option<u64>) -> String {
    let Some(since) = since else {
        return "unknown".to_string();
    };
    let secs = now.saturating_sub(since);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

fn render(status: &SyncStatus, json: bool) {
    if json {
        match serde_json::to_string_pretty(status) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("cannot serialize status: {error}"),
        }
        return;
    }

    // Every verdict below inherits the age of the state document, so the age is
    // computed once and printed next to the claim it qualifies.
    let now = now_secs();

    println!("{} checkout {}", "→".cyan(), short(&status.local_revision));
    match &status.remote_revision {
        Some(remote) => println!("  remote    {}", short(remote)),
        None => println!("  remote    {}", "unreadable".yellow()),
    }
    match &status.applied_revision {
        Some(applied) => println!("  applied   {}", short(applied)),
        None => println!("  applied   {}", "never".yellow()),
    }
    match status.observed_at {
        Some(at) => println!(
            "  observed  {}",
            format!("{} ago", age(now, Some(at))).dimmed()
        ),
        None => println!("  observed  {}", "unknown".dimmed()),
    }
    match status.behind {
        Some(true) => println!("  {}", "fleet is behind the remote revision".yellow()),
        Some(false) => println!("  {}", "fleet is at the remote revision".green()),
        None => println!("  {}", "cannot tell: the remote is unreadable".dimmed()),
    }
    if status.backoff_secs > 0 {
        println!(
            "  {} backing off for {}s",
            "!".yellow(),
            status.backoff_secs
        );
    }
    if let Some(error) = &status.state_error {
        println!("  {} state unreadable: {error}", "!".yellow());
    }

    // Host conditions are the reconciler's memory. They are printed even when
    // this call did not probe anything: "offline for an hour" is the answer to
    // "why is this host not at the revision", and re-probing would not change
    // it. A condition that needs no action (`reachable`) stays quiet.
    for entry in status.conditions.iter().filter(|e| e.label != "reachable") {
        let mut line = format!(
            "  {} {} {} for {}",
            "!".yellow(),
            entry.host.bold(),
            entry.label.yellow(),
            age(now, entry.since)
        );
        if let Some(detail) = &entry.detail {
            line.push_str(&format!(", {detail}"));
        }
        // A retry that is already due says nothing useful; printing "in 0s"
        // on every line would train the reader to ignore the field.
        if let Some(seconds) = entry.next_probe_in.filter(|seconds| *seconds > 0) {
            line.push_str(&format!(", next probe in {seconds}s"));
        }
        println!("{line}");
    }

    for host in &status.hosts {
        let label = if host.unreachable {
            "unreachable".yellow()
        } else if host.drifted {
            "drifted".yellow()
        } else {
            "current".green()
        };
        println!("  {} {}", host.host.bold(), label);
    }
    if status.hosts.is_empty() {
        println!("  {}", "drift not checked (pass --drift)".dimmed());
    }
}

fn short(revision: &str) -> &str {
    revision.get(..12).unwrap_or(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_picks_the_largest_useful_unit() {
        assert_eq!(age(1_000, Some(970)), "30s");
        assert_eq!(age(1_000, Some(700)), "5m");
        assert_eq!(age(100_000, Some(92_800)), "2h");
        assert_eq!(age(200_000, Some(27_200)), "2d");
    }

    #[test]
    fn age_never_underflows_on_a_clock_that_jumped_back() {
        assert_eq!(age(100, Some(500)), "0s");
    }

    #[test]
    fn a_missing_timestamp_is_reported_as_unknown() {
        assert_eq!(age(100, None), "unknown");
    }
}
