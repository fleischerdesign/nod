//! Systemd health checker adapter: verifies a local host post-activation
//! (ADR-003 observability).
//!
//! Probes `systemctl is-system-running` (the system state) and
//! `systemctl --failed` (the failed-unit list). A host is healthy only when
//! the system is up and no unit is in a failed state.
//!
//! Systemd is a *local* surface: this adapter verifies local hosts only and
//! raises a typed health error for remote hosts (the SSH deployer owns those
//! remote probes).

use async_trait::async_trait;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::ports::health_checker::HealthCheckerPort;

/// Verifies a live local system via the systemd probes.
pub struct SystemdHealthChecker;

impl SystemdHealthChecker {
    /// Builds the adapter.
    pub fn new() -> Self {
        Self
    }

    /// Reaches a health verdict from a reported systemd state and the
    /// failed-unit rows. `running`/`degraded` count as "the system is up";
    /// any failed unit flips the verdict to unhealthy.
    pub fn healthy(state: &str, failed_units: &[String]) -> bool {
        let up = state == "running" || state == "degraded";
        up && failed_units.is_empty()
    }

    /// Extracts the bulleted failed-unit rows from `systemctl --failed` output.
    /// Table rows carry a leading bullet; the header row, legend and
    /// "N units listed." summary carry none and are ignored.
    pub fn failed_units(output: &str) -> Vec<String> {
        let mut failed = Vec::new();
        for line in output.split("\n") {
            let trimmed = line.trim();
            if trimmed.contains("●") && !trimmed.contains("units listed") {
                failed.push(trimmed.to_string());
            }
        }
        failed
    }
}

#[async_trait]
impl HealthCheckerPort for SystemdHealthChecker {
    async fn verify_health(&self, host: &HostEntity) -> Result<bool, NodError> {
        if !host.is_local {
            return Err(NodError::healthcheck(format!(
                "host '{}' is remote; the systemd checker only verifies local systems",
                host.name
            )));
        }

        let system_state = Command::new("systemctl")
            .args(["is-system-running"])
            .output()
            .await;
        if system_state.is_err() {
            return Err(NodError::healthcheck("failed to launch `systemctl`"));
        }
        let system_state = system_state.unwrap();
        let state = String::from_utf8_lossy(&system_state.stdout);
        let state = state.trim();

        let failed = Command::new("systemctl")
            .args(["--failed", "--no-pager"])
            .output()
            .await;
        if failed.is_err() {
            return Err(NodError::healthcheck("failed to launch `systemctl --failed`"));
        }
        let failed = failed.unwrap();
        let failed_output = String::from_utf8_lossy(&failed.stdout);

        Ok(Self::healthy(&state, &Self::failed_units(&failed_output)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_system_without_failed_units_is_healthy() {
        assert!(SystemdHealthChecker::healthy("running", &vec![]));
        assert!(SystemdHealthChecker::healthy("degraded", &vec![]));
    }

    #[test]
    fn failed_units_flip_the_verdict_unhealthy() {
        let failed = vec!["● foo.service failed".to_string()];
        assert!(!SystemdHealthChecker::healthy("running", &failed));
        assert!(!SystemdHealthChecker::healthy("degraded", &failed));
    }

    #[test]
    fn non_running_states_are_unhealthy() {
        for state in ["offline", "maintenance", "stopping", "unknown", ""] {
            assert!(
                !SystemdHealthChecker::healthy(state, &vec![]),
                "state={}",
                state
            );
        }
    }

    #[test]
    fn failed_units_parses_bulleted_rows_and_skips_summary() {
        let output = "UNIT  LOAD  ACTIVE SUB  DESCRIPTION\n  ● foo.service loaded failed failed Some service\n  ● bar.service loaded failed failed Another service\n0 failed units listed.\n";
        let units = SystemdHealthChecker::failed_units(output);
        assert_eq!(units.len(), 2);
        assert!(units[0].contains("foo.service"));
        assert!(units[1].contains("bar.service"));
    }

    #[test]
    fn no_bulleted_rows_means_no_failed_units() {
        assert!(SystemdHealthChecker::failed_units("0 loaded units listed.").is_empty());
    }
}