//! Shared post-activation probe (ADR-003): failed systemd units.
//!
//! The check is deliberately one question - is any unit failed? - because that
//! is the signal every NixOS host can answer, locally and over SSH. Richer
//! probes stay in the fleet's `options.nod.healthChecks` configuration; this
//! module only turns the answer into a verdict.

use std::time::Duration;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Command that lists failed units without a legend or colour.
pub const FAILED_UNITS_COMMAND: &str = "systemctl --failed --no-legend --plain";

/// Parses `systemctl --failed --no-legend --plain` output into unit names.
pub fn failed_units(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|unit| !unit.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether post-activation verification is enabled for `host`.
pub fn enabled(host: &HostEntity) -> bool {
    host.nod_config.health_checks.enable.unwrap_or(true)
}

/// Whether failed units make `host` unhealthy.
pub fn checks_failed_units(host: &HostEntity) -> bool {
    host.nod_config
        .health_checks
        .systemd
        .check_failed_units
        .unwrap_or(true)
}

/// Probe timeout for `host`.
pub fn timeout(host: &HostEntity) -> Duration {
    Duration::from_secs(u64::from(
        host.nod_config.health_checks.timeout_secs.unwrap_or(60),
    ))
}

/// Turns raw `systemctl --failed` output into a health verdict.
pub fn verdict(host: &HostEntity, output: &str) -> Result<(), NodError> {
    if !enabled(host) || !checks_failed_units(host) {
        return Ok(());
    }
    let failed = failed_units(output);
    if failed.is_empty() {
        Ok(())
    } else {
        Err(NodError::health_check(format!(
            "host {} has failed units: {}",
            host.name,
            failed.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostEntity {
        HostEntity::new("atlas", "10.0.0.8", false)
    }

    #[test]
    fn parses_unit_names_from_systemctl_output() {
        let output = "nginx.service loaded failed failed A web server\nsshd.service loaded failed failed OpenSSH\n";
        assert_eq!(
            failed_units(output),
            vec!["nginx.service".to_string(), "sshd.service".to_string()]
        );
    }

    #[test]
    fn empty_output_is_healthy() {
        assert!(verdict(&host(), "").is_ok());
    }

    #[test]
    fn failed_units_are_unhealthy_and_named() {
        let error = verdict(&host(), "nginx.service loaded failed failed\n").unwrap_err();
        assert!(error.to_string().contains("nginx.service"), "got: {error}");
    }

    #[test]
    fn verification_can_be_disabled_per_host() {
        let mut host = host();
        host.nod_config.health_checks.enable = Some(false);
        assert!(verdict(&host, "nginx.service loaded failed failed\n").is_ok());
    }

    #[test]
    fn failed_unit_check_can_be_disabled_per_host() {
        let mut host = host();
        host.nod_config.health_checks.systemd.check_failed_units = Some(false);
        assert!(verdict(&host, "nginx.service loaded failed failed\n").is_ok());
    }

    #[test]
    fn timeout_defaults_to_one_minute_and_honours_configuration() {
        let host = host();
        assert_eq!(timeout(&host), Duration::from_secs(60));

        let mut configured = host;
        configured.nod_config.health_checks.timeout_secs = Some(5);
        assert_eq!(timeout(&configured), Duration::from_secs(5));
    }
}
