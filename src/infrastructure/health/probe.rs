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

/// Command that prints the units systemd is restarting on its own.
///
/// `systemctl --failed` cannot answer this: a unit with `Restart=on-failure`
/// that keeps dying is `activating`/`auto-restart`, not `failed`, so a restart
/// loop is invisible to the failed-unit probe (ADR-029).
///
/// The trailing `true` is load-bearing: without it the pipeline exits with the
/// status of the last test, so a host with nothing restarting would report a
/// failed probe instead of a clean one.
pub const RESTARTING_UNITS_COMMAND: &str = concat!(
    "systemctl list-units --all --plain --no-legend --state=activating --no-pager | ",
    "while read -r unit _; do ",
    "[ \"$(systemctl show -p SubState --value \"$unit\")\" = auto-restart ] && echo \"$unit\"; ",
    "done; true"
);

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

/// Whether a restart loop makes `host` unhealthy.
pub fn checks_restart_loops(host: &HostEntity) -> bool {
    host.nod_config
        .health_checks
        .systemd
        .check_restart_loops
        .unwrap_or(true)
}

/// Unit names from the restarting-units probe, one per line.
pub fn restarting_units(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Probe timeout for `host`.
pub fn timeout(host: &HostEntity) -> Duration {
    Duration::from_secs(u64::from(
        host.nod_config.health_checks.timeout_secs.unwrap_or(60),
    ))
}

/// Turns the two probes into a health verdict.
///
/// A failed unit and a restart loop are separate answers because they are
/// separate observations; either one makes the host unhealthy, and neither can
/// be inferred from the other.
pub fn verdict_with_restarts(
    host: &HostEntity,
    failed_output: &str,
    restarting_output: &str,
) -> Result<(), NodError> {
    if !enabled(host) {
        return Ok(());
    }
    verdict(host, failed_output)?;
    if !checks_restart_loops(host) {
        return Ok(());
    }
    let restarting = restarting_units(restarting_output);
    if restarting.is_empty() {
        Ok(())
    } else {
        Err(NodError::health_check(format!(
            "host {} is restarting units: {}",
            host.name,
            restarting.join(", ")
        )))
    }
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
    fn restarting_units_are_read_one_per_line() {
        assert_eq!(
            restarting_units("crowdsec.service\n\n  foo.service  \n"),
            vec!["crowdsec.service".to_string(), "foo.service".to_string()]
        );
        assert!(restarting_units("").is_empty());
    }

    #[test]
    fn a_restart_loop_fails_the_verdict() {
        let error = verdict_with_restarts(&host(), "", "crowdsec.service\n").unwrap_err();
        assert!(
            error.to_string().contains("crowdsec.service"),
            "got: {error}"
        );
        assert!(verdict_with_restarts(&host(), "", "").is_ok());
    }

    #[test]
    fn restart_loop_check_can_be_disabled_per_host() {
        let mut host = host();
        host.nod_config.health_checks.systemd.check_restart_loops = Some(false);

        assert!(verdict_with_restarts(&host, "", "crowdsec.service\n").is_ok());
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
