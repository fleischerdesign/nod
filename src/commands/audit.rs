//! `nod audit` command: read the recorded deployment audit trail (ADR-003
//! observability). Backed by `AuditLogUseCase` over `AuditStorePort`.

use colored::Colorize;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::audit_log::AuditLogUseCase;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> Result<(), NodError> {
    // The audit store is bound by `main` (a single, explicit site) because it
    // is an opt-in service: `production` builds the base graph without it.
    let ctx = Arc::new(ctx);
    let use_case = AuditLogUseCase::new(ctx);
    let entries = use_case.execute(target, limit).await?;

    if json {
        println!("{}", serde_json::to_string(&entries).unwrap());
        return Ok(());
    }

    if entries.is_empty() {
        println!("{}", "No deployment history recorded yet.".dimmed());
        return Ok(());
    }

    println!(
        "\n{:<20} {:<14} {:<12}",
        "HOST".bold(),
        "OUTCOME".bold(),
        "RECORDED (UTC)".bold()
    );
    for entry in entries {
        let outcome = entry.outcome.clone();
        let colored = if outcome == "completed" {
            outcome.green()
        } else {
            outcome.yellow()
        };
        println!(
            "{:<20} {:<14} {}",
            entry.host_name, colored, entry.recorded_at
        );
    }

    Ok(())
}
