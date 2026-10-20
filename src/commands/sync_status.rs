//! `nod sync-status` command: read-only view of the reconciler (ADR-028).

use std::path::Path;
use std::sync::Arc;

use colored::Colorize;

use crate::application::context::AppContext;
use crate::application::use_cases::sync_status::SyncStatusUseCase;
use crate::commands::wiring;
use crate::domain::errors::NodError;
use crate::domain::sync::SyncStatus;
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

fn render(status: &SyncStatus, json: bool) {
    if json {
        match serde_json::to_string_pretty(status) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("cannot serialize status: {error}"),
        }
        return;
    }

    println!("{} checkout {}", "→".cyan(), short(&status.local_revision));
    match &status.remote_revision {
        Some(remote) => println!("  remote    {}", short(remote)),
        None => println!("  remote    {}", "unreadable".yellow()),
    }
    match &status.applied_revision {
        Some(applied) => println!("  applied   {}", short(applied)),
        None => println!("  applied   {}", "never".yellow()),
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
    if !status.unreachable.is_empty() {
        println!(
            "  {} unreachable: {}",
            "!".yellow(),
            status.unreachable.join(", ")
        );
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
