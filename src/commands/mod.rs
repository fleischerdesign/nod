pub mod audit;
pub mod boot;
pub mod bootstrap;
pub mod build;
pub mod cache;
pub mod check;
pub mod copy;
pub mod daemon;
pub mod dashboard;
pub mod diff;
pub mod drift;
pub mod eval;
pub mod exec;
pub mod export;
pub mod gc;
pub mod generations;
pub mod graph;
pub mod info;
pub mod init;
pub mod inputs;
pub mod iso;
pub mod lifecycle;
pub mod metadata;
pub mod plan;
pub mod reboot;
pub mod repl;
pub mod rollback;
pub mod secret;
pub mod ssh;
pub mod status;
pub mod store;
pub mod switch;
pub mod sync;
pub mod test;
pub mod update;
pub mod watch;
pub mod wiring;

use colored::Colorize;

use crate::application::use_cases::deploy_fleet::FleetSummary;
use crate::domain::errors::NodError;

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

/// Renders the fleet result and turns an unsuccessful host into a typed error, so the process exit
/// status reflects the deployment (ADR-027). Every lifecycle command (`switch`/`test`/`boot`/
/// `build`) calls this one function: a host that did not reach a good terminal state fails the
/// command, even with `--on-error continue`, because the failures happened regardless of how far the
/// run proceeded.
pub(crate) fn report_summary(summary: &FleetSummary, quiet: bool) -> Result<(), NodError> {
    if !quiet {
        render_summary(summary);
    }
    let unsuccessful = summary
        .outcomes
        .iter()
        .filter(|outcome| !outcome.ok)
        .count();
    if unsuccessful > 0 {
        return Err(NodError::deployment(format!(
            "{} of {} host(s) did not complete successfully",
            unsuccessful,
            summary.outcomes.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::report_summary;
    use crate::application::pipeline::state_machine::DeploymentState;
    use crate::application::use_cases::deploy_fleet::{FleetSummary, HostOutcome};
    use crate::domain::errors::NodError;

    fn summary(states: &[DeploymentState]) -> FleetSummary {
        FleetSummary {
            outcomes: states
                .iter()
                .enumerate()
                .map(|(index, state)| HostOutcome::new(format!("host-{index}"), state.clone()))
                .collect(),
            aborted: false,
        }
    }

    #[test]
    fn report_summary_accepts_a_completed_fleet() {
        assert!(report_summary(&summary(&[DeploymentState::Completed]), true).is_ok());
    }

    #[test]
    fn report_summary_accepts_a_dry_run_preview() {
        assert!(report_summary(&summary(&[DeploymentState::Prepared]), true).is_ok());
    }

    #[test]
    fn report_summary_fails_a_failed_host() {
        let result = report_summary(
            &summary(&[DeploymentState::Completed, DeploymentState::Failed]),
            true,
        );
        assert!(matches!(result, Err(NodError::Deployment { .. })));
    }

    #[test]
    fn report_summary_fails_a_rolled_back_host() {
        let result = report_summary(&summary(&[DeploymentState::RolledBack]), true);
        assert!(matches!(result, Err(NodError::Deployment { .. })));
    }
}
