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
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemdHealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemdHealthChecker {
    /// Reaches a health verdict from a reported systemd state and the
    /// failed-unit rows. `running`/`degraded` count as "the system is up";
    /// any failed unit flips the verdict to unhealthy.
    pub fn healthy(state: &str, failed_units: &[String]) -> bool {
        let up = state == "running" || state == "degraded";
        up && failed_units.is_empty()
    }

    /// Extracts the failed-unit names from `systemctl --failed` output by
    /// reading the `UNIT`, `ACTIVE` and `SUB` columns (AC2) instead of
    /// relying on the leading bullet glyph that systemd omits when colour is
    /// disabled on a pipe. A row counts as a failed unit when its `ACTIVE` or
    /// `SUB` column reads `failed`; the header and "N units listed." summary
    /// rows are skipped. The summary skip keys off the leading token parsing
    /// as a count, not a leading digit, so a failed unit whose name starts
    /// with a digit (e.g. `10gig.service`) is not dropped.
    pub fn failed_units(output: &str) -> Vec<String> {
        let mut failed = Vec::new();
        for line in output.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 {
                continue;
            }
            // Skip the header row and the "N units listed." summary. The
            // summary is recognised by its first token parsing as a count and
            // the remainder reading as a "... units listed." row, not by a
            // leading digit.
            if cols[0] == "UNIT"
                || (cols[0].parse::<usize>().is_ok()
                    && cols[1..].join(" ").ends_with("units listed."))
            {
                continue;
            }
            if cols[2] == "failed" || cols[3] == "failed" {
                failed.push(cols[0].to_string());
            }
        }
        failed
    }

    /// Pure verdict over the local systemd probes (AC2). `state` is
    /// `is-system-running`'s output, `failed_units` the parsed failed names
    /// and `required_active[i]` whether the i-th `required_units` entry is
    /// active. Unhealthy when the system is down, any unit failed, or any
    /// required unit is not active.
    fn healthy_with_required(
        state: &str,
        failed_units: &[String],
        required_active: &[bool],
    ) -> bool {
        if required_active.iter().any(|active| !*active) {
            return false;
        }
        Self::healthy(state, failed_units)
    }
}

#[async_trait]
impl HealthCheckerPort for SystemdHealthChecker {
    async fn verify_health(&self, host: &HostEntity) -> Result<bool, NodError> {
        if !host.is_local {
            return Err(NodError::health_check(format!(
                "host '{}' is remote; the systemd checker only verifies local systems",
                host.name
            )));
        }

        // AC2: an explicitly-disabled health check short-circuits to healthy.
        if host.nod_config.health_checks.enable == Some(false) {
            return Ok(true);
        }

        let system_state = Command::new("systemctl")
            .args(["is-system-running"])
            .output()
            .await;
        if system_state.is_err() {
            return Err(NodError::health_check("failed to launch `systemctl`"));
        }
        let system_state = system_state.unwrap();
        let state = String::from_utf8_lossy(&system_state.stdout);
        let state = state.trim();

        let failed = Command::new("systemctl")
            .args(["--failed", "--no-pager"])
            .output()
            .await;
        if failed.is_err() {
            return Err(NodError::health_check(
                "failed to launch `systemctl --failed`",
            ));
        }
        let failed = failed.unwrap();
        let failed_output = String::from_utf8_lossy(&failed.stdout);
        let failed_units = Self::failed_units(&failed_output);

        // AC2: every configured required unit must be active.
        let mut required_active = Vec::new();
        if let Some(required) = &host.nod_config.health_checks.systemd.required_units {
            for unit in required {
                let status = Command::new("systemctl")
                    .args(["is-active", "--quiet", unit])
                    .status()
                    .await;
                required_active.push(matches!(status, Ok(s) if s.success()));
            }
        }

        Ok(Self::healthy_with_required(
            state,
            &failed_units,
            &required_active,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostEntity;

    #[test]
    fn running_system_without_failed_units_is_healthy() {
        assert!(SystemdHealthChecker::healthy("running", &[]));
        assert!(SystemdHealthChecker::healthy("degraded", &[]));
    }

    #[test]
    fn failed_units_flip_the_verdict_unhealthy() {
        let failed = vec!["foo.service".to_string()];
        assert!(!SystemdHealthChecker::healthy("running", &failed));
        assert!(!SystemdHealthChecker::healthy("degraded", &failed));
    }

    #[test]
    fn non_running_states_are_unhealthy() {
        for state in ["offline", "maintenance", "stopping", "unknown", ""] {
            assert!(
                !SystemdHealthChecker::healthy(state, &[]),
                "state={}",
                state
            );
        }
    }

    #[test]
    fn failed_units_parses_column_output_without_glyph() {
        // Real systemd `--failed` column output with colour/glyphs disabled.
        let output = "  UNIT                     LOAD   ACTIVE SUB    DESCRIPTION\n  foo.service              loaded failed failed Some service failed\n  bar.service              loaded failed failed Another service\n0 failed units listed.\n";
        let units = SystemdHealthChecker::failed_units(output);
        assert_eq!(
            units,
            vec!["foo.service".to_string(), "bar.service".to_string()]
        );
    }

    #[test]
    fn no_failed_rows_means_no_failed_units() {
        assert!(SystemdHealthChecker::failed_units("0 loaded units listed.").is_empty());
        assert!(SystemdHealthChecker::failed_units("").is_empty());
    }

    #[test]
    fn digit_leading_failed_unit_is_detected_while_summary_is_skipped() {
        // A failed unit whose name starts with a digit must not be dropped by
        // the summary-line heuristic (AC2), while the "N units listed."
        // summary row is still skipped.
        let output = "  UNIT                      LOAD   ACTIVE SUB    DESCRIPTION\n  10gig.service             loaded failed failed Big service\n3 failed units listed.\n";
        let units = SystemdHealthChecker::failed_units(output);
        assert_eq!(units, vec!["10gig.service".to_string()]);
    }

    #[test]
    fn required_unit_not_active_yields_unhealthy() {
        assert!(!SystemdHealthChecker::healthy_with_required(
            "running",
            &[],
            &[false]
        ));
        assert!(!SystemdHealthChecker::healthy_with_required(
            "running",
            &["dep.service".to_string()],
            &[true, false]
        ));
    }

    #[test]
    fn empty_required_units_fall_back_to_base_semantics() {
        assert!(SystemdHealthChecker::healthy_with_required(
            "running",
            &[],
            &[]
        ));
        assert!(!SystemdHealthChecker::healthy_with_required(
            "offline",
            &[],
            &[]
        ));
    }

    #[tokio::test]
    async fn disabled_health_check_short_circuits_healthy() {
        let mut host = HostEntity::new("local-box", "local-box", true);
        host.nod_config.health_checks.enable = Some(false);
        let result = SystemdHealthChecker.verify_health(&host).await;
        assert!(matches!(result, Ok(true)));
    }
}
