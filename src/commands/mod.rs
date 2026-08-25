pub mod audit;
pub mod boot;
pub mod build;
pub mod check;
pub mod copy;
pub mod dashboard;
pub mod diff;
pub mod drift;
pub mod exec;
pub mod gc;
pub mod generations;
pub mod inputs;
pub mod lifecycle;
pub mod metadata;
pub mod plan;
pub mod rollback;
pub mod ssh;
pub mod status;
pub mod switch;
pub mod test;
pub mod update;
pub mod wiring;

use colored::Colorize;

use crate::application::use_cases::deploy_fleet::FleetSummary;

/// Renders the fleet result for the operator (ADR-008). The single shared
/// implementation used by the deploy lifecycle commands (`switch`/`test`/
/// `boot`/`build`); it always renders per-outcome failure detail when present.
pub(crate) fn render_summary(summary: &FleetSummary) {
    for outcome in summary.outcomes.clone() {
        let label = format!("[{}]", outcome.state.to_str());
        let colored = if outcome.ok {
            label.green()
        } else {
            label.red()
        };
        println!("  {} {}", outcome.host_name.bold(), colored);
        if let Some(failure) = outcome.failure.as_deref() {
            println!("      {}", failure.red().dimmed());
        }
    }
    println!(
        "\n  {}",
        format!(
            "{} succeeded, {} rolled back, {} failed (aborted: {})",
            summary.succeeded(),
            summary.rolled_back(),
            summary.failed(),
            summary.aborted
        )
        .dimmed()
    );
}
