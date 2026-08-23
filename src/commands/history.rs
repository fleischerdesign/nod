//! `nod history` command: read the recorded deployment audit trail (ADR-003
//! observability). Backed by `AuditLogUseCase` over `HistoryStorePort`.

use colored::Colorize;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::audit_log::AuditLogUseCase;
use crate::domain::errors::NodError;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;
use crate::infrastructure::storage::json_history_store::JsonHistoryStore;

pub async fn execute(target: Option<&str>, limit: Option<usize>, json: bool) -> Result<(), NodError> {
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_history_store(Arc::new(JsonHistoryStore::new()));

    let use_case = AuditLogUseCase::new(Arc::new(ctx));
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
        println!("{:<20} {:<14} {}", entry.host_name, colored, entry.recorded_at);
    }

    Ok(())
}