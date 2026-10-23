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
/// The substate is read from the `--plain` table's fourth column rather than
/// from one `systemctl show` per unit: a per-unit call can block while a unit
/// is still activating, and a probe that hangs is worse than one that misses.
///
/// The trailing `true` keeps the exit status clean when nothing matches.
pub const RESTARTING_UNITS_COMMAND: &str = concat!(
    "systemctl list-units --all --plain --no-legend --state=activating --no-pager ",
    "| awk '$4 == \"auto-restart\" { print $1 }'; true"
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

/// Whether the units an activation touched must be running (ADR-029).
pub fn checks_changed_units(host: &HostEntity) -> bool {
    host.nod_config
        .health_checks
        .systemd
        .check_changed_units
        .unwrap_or(true)
}

/// Command that reports whether the host asks for a reboot.
///
/// `switch-to-configuration` writes `/run/reboot-required` when the new
/// generation cannot take effect without one; the answer is printed rather
/// than signalled so a missing marker is a clean exit, not a failed probe.
pub const REBOOT_REQUIRED_COMMAND: &str =
    "if [ -e /run/reboot-required ]; then echo yes; else echo no; fi";

/// Whether the reboot marker was reported.
pub fn reboot_required(output: &str) -> bool {
    output.trim() == "yes"
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

/// Quotes a value for a POSIX shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The shell loop that judges units: declared ones and the ones an activation
/// touched.
///
/// `__PAIRS__` is replaced with `<unit> <counter>` words. The counter flag keeps
/// restart accounting on the declared units only: the activation restarts the
/// units it touched by design, and counting that as a regression would punish
/// every deployment that does its job.
const UNIT_LOOP: &str = r#"for entry in __PAIRS__; do
  unit=${entry% *}
  counter=${entry#* }
  load=$(systemctl show "$unit" --property=LoadState --value 2>/dev/null)
  if [ "$load" = not-found ]; then
    echo "unit $unit: not found"
    continue
  fi
  state=$(systemctl show "$unit" --property=ActiveState --value 2>/dev/null)
  sub=$(systemctl show "$unit" --property=SubState --value 2>/dev/null)
  result=$(systemctl show "$unit" --property=Result --value 2>/dev/null)
  restarts=$(systemctl show "$unit" --property=NRestarts --value 2>/dev/null)
  if [ "$counter" = 1 ]; then
    echo "restart $unit ${restarts:-0}"
  fi
  if [ "$state" = activating ] && [ "$sub" != auto-restart ]; then
    for _ in 1 2 3 4 5; do
      sleep 1
      state=$(systemctl show "$unit" --property=ActiveState --value 2>/dev/null)
      [ "$state" = activating ] || break
    done
  fi
  case "$state" in
    active) ;;
    inactive) [ "$result" = success ] || echo "unit $unit: inactive (${result:-unknown})" ;;
    "") echo "unit $unit: unknown" ;;
    *) echo "unit $unit: $state${sub:+ ($sub)}" ;;
  esac
done"#;

/// Deduplicates unit names while keeping the caller's order.
fn unique(units: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    units
        .into_iter()
        .filter(|unit| seen.insert(unit.clone()))
        .collect()
}

/// The shell command that checks declared units and probes, when any are
/// configured.
///
/// `touched` are the units the activation reported; they are judged by state
/// only. One command, one channel: every problem is a line, and the snapshot
/// keeps the line as the reason. A new probe kind does not change the snapshot
/// type.
pub fn extra_probe_command(host: &HostEntity, touched: &[String]) -> Option<String> {
    let health = &host.nod_config.health_checks;
    let mut parts = Vec::new();

    let declared = if health.systemd.check_running.unwrap_or(true) {
        health
            .systemd
            .required_units
            .as_deref()
            .unwrap_or(&[])
            .to_vec()
    } else {
        Vec::new()
    };

    let mut judged = declared.clone();
    if checks_changed_units(host) {
        judged.extend(touched.iter().cloned());
    }
    let judged = unique(judged);

    if !judged.is_empty() {
        let pairs = judged
            .iter()
            .map(|unit| {
                let counter = u8::from(declared.contains(unit));
                shell_quote(&format!("{unit} {counter}"))
            })
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(UNIT_LOOP.replace("__PAIRS__", &pairs));
    }

    let ports = health.tcp_ports.as_deref().unwrap_or(&[]);
    if !ports.is_empty() {
        let mut checks = String::new();
        for port in ports {
            checks.push_str(&format!(
                "  ss -ltnH \"sport = :{port}\" 2>/dev/null | grep -q . || echo \"tcp port {port}: not listening\"\n"
            ));
        }
        parts.push(format!(
            "if command -v ss >/dev/null 2>&1; then\n{checks}else\n  echo \"tcp: ss is not installed\"\nfi"
        ));
    }

    let http = health.http_probes.as_deref().unwrap_or(&[]);
    if !http.is_empty() {
        let mut probes = String::new();
        for probe in http {
            let Some(url) = probe.url.as_deref() else {
                continue;
            };
            let expected = probe.expected_status.unwrap_or(200);
            let timeout = probe.timeout_secs.unwrap_or(5);
            probes.push_str(&format!(
                "  code=$(curl -s -o /dev/null -w '%{{http_code}}' --max-time {timeout} {url} 2>/dev/null)\n  [ \"$code\" = \"{expected}\" ] || echo \"http {url}: expected {expected}, got ${{code:-none}}\"\n",
                url = shell_quote(url),
                expected = expected,
                timeout = timeout
            ));
        }
        if !probes.is_empty() {
            parts.push(format!(
                "if command -v curl >/dev/null 2>&1; then\n{probes}else\n  echo \"http: curl is not installed\"\nfi"
            ));
        }
    }

    let custom = health.custom_probes.as_deref().unwrap_or(&[]);
    for probe in custom {
        let Some(command) = probe.command.as_deref() else {
            continue;
        };
        let name = probe.name.as_deref().unwrap_or(command);
        let timeout = probe.timeout_secs.unwrap_or(5);
        parts.push(format!(
            "timeout {timeout} sh -c {command} >/dev/null 2>&1 || echo \"probe {name}: failed\"",
            command = shell_quote(command),
            name = shell_quote(name),
        ));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Builds the observation the deployment compares, honouring the host's
/// switches.
///
/// A disabled check contributes nothing to the snapshot, which is what makes
/// opting out mean "do not judge this" rather than "judge it as clean".
pub fn snapshot(
    host: &HostEntity,
    failed_output: &str,
    restarting_output: &str,
    extra_output: &str,
) -> crate::domain::health::HealthSnapshot {
    use crate::domain::health::HealthSnapshot;

    if !enabled(host) {
        return HealthSnapshot::clean();
    }

    let mut restarts = std::collections::BTreeMap::new();
    let mut extra_problems = Vec::new();
    for line in extra_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(rest) = line.strip_prefix("restart ") {
            if let Some((unit, count)) = rest.split_once(' ') {
                if let Ok(count) = count.trim().parse::<u32>() {
                    restarts.insert(unit.to_string(), count);
                    continue;
                }
            }
        }
        extra_problems.push(line.to_string());
    }

    HealthSnapshot {
        failed_units: if checks_failed_units(host) {
            failed_units(failed_output)
        } else {
            Vec::new()
        },
        restarting_units: if checks_restart_loops(host) {
            restarting_units(restarting_output)
        } else {
            Vec::new()
        },
        extra_problems,
        restarts,
    }
}

/// Probe timeout for `host`.
pub fn timeout(host: &HostEntity) -> Duration {
    Duration::from_secs(u64::from(
        host.nod_config.health_checks.timeout_secs.unwrap_or(60),
    ))
}

/// Allowed automatic-restart increases for `host` (ADR-029).
pub fn max_restarts(host: &HostEntity) -> u32 {
    host.nod_config
        .health_checks
        .systemd
        .max_restarts
        .unwrap_or(0)
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
    use crate::domain::config::{CustomProbeConfig, HttpProbeConfig};

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
    fn the_restart_probe_reads_the_table_instead_of_calling_per_unit() {
        // A per-unit `systemctl show` can block while a unit is activating,
        // and a probe that hangs costs the whole verification (measured on
        // remote hosts that timed out at 60 s).
        assert!(!RESTARTING_UNITS_COMMAND.contains("while read"));
        assert!(!RESTARTING_UNITS_COMMAND.contains("systemctl show"));
        assert!(RESTARTING_UNITS_COMMAND.contains("awk"));
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

    #[test]
    fn no_extra_probes_means_no_command() {
        assert!(extra_probe_command(&host(), &[]).is_none());
    }

    #[test]
    fn declared_units_are_checked() {
        let mut host = host();
        host.nod_config.health_checks.systemd.required_units =
            Some(vec!["web.service".to_string()]);

        let command = extra_probe_command(&host, &[]).expect("a declared unit needs a probe");
        assert!(command.contains("systemctl show"));
        assert!(command.contains("NRestarts"));
        assert!(command.contains("web.service"));
    }

    #[test]
    fn declared_units_can_be_disabled() {
        let mut host = host();
        host.nod_config.health_checks.systemd.check_running = Some(false);
        host.nod_config.health_checks.systemd.required_units =
            Some(vec!["web.service".to_string()]);

        assert!(extra_probe_command(&host, &[]).is_none());
    }

    #[test]
    fn touched_units_are_judged_without_a_restart_counter() {
        let host = host();
        let command = extra_probe_command(&host, &["app.socket".to_string()])
            .expect("a touched unit needs a probe");

        assert!(command.contains("'app.socket 0'"));
        assert!(command.contains("LoadState"));
    }

    #[test]
    fn a_touched_unit_that_is_also_declared_keeps_its_counter() {
        let mut host = host();
        host.nod_config.health_checks.systemd.required_units =
            Some(vec!["web.service".to_string()]);

        let command = extra_probe_command(&host, &["web.service".to_string()])
            .expect("a declared unit needs a probe");

        assert_eq!(command.matches("'web.service ").count(), 1);
        assert!(command.contains("'web.service 1'"));
    }

    #[test]
    fn touched_units_can_be_disabled() {
        let mut host = host();
        host.nod_config.health_checks.systemd.check_changed_units = Some(false);

        assert!(extra_probe_command(&host, &["app.socket".to_string()]).is_none());
    }

    /// Writes an executable double into its own temporary directory.
    fn shim(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("nod-probe-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    /// The probe's `PATH` with `dir` in front, so a double wins over the host.
    fn path_with(dir: &std::path::Path) -> String {
        format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    /// Proves the generated loop, not the string that builds it.
    ///
    /// A `systemctl` double answers for three units: one that must be running,
    /// one the activation touched and that failed, and one that does not exist.
    /// The loop has to keep the healthy unit quiet, report the failed one by
    /// state, and name a missing unit instead of passing it silently.
    #[tokio::test]
    async fn the_generated_loop_separates_missing_from_failed() {
        let dir = shim(
            "systemctl",
            r#"#!/bin/sh
unit=$2
prop=${3#--property=}
case "$unit:$prop" in
  web.service:LoadState) echo loaded ;;
  web.service:ActiveState) echo active ;;
  web.service:SubState) echo running ;;
  web.service:Result) echo success ;;
  web.service:NRestarts) echo 2 ;;
  app.socket:LoadState) echo loaded ;;
  app.socket:ActiveState) echo failed ;;
  app.socket:SubState) echo failed ;;
  app.socket:Result) echo exit-code ;;
  app.socket:NRestarts) echo 0 ;;
  gone.service:LoadState) echo not-found ;;
  *) echo "" ;;
esac
"#,
        );

        let mut host = host();
        host.nod_config.health_checks.systemd.required_units =
            Some(vec!["web.service".to_string(), "gone.service".to_string()]);
        let command = extra_probe_command(&host, &["app.socket".to_string()])
            .expect("declared and touched units need a probe");

        let output = tokio::process::Command::new("sh")
            .args(["-c", &command])
            .env("PATH", path_with(&dir))
            .output()
            .await
            .expect("the probe shell should run");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        assert!(lines.contains(&"restart web.service 2"), "got: {stdout}");
        assert!(
            lines.contains(&"unit gone.service: not found"),
            "got: {stdout}"
        );
        assert!(
            lines.contains(&"unit app.socket: failed (failed)"),
            "got: {stdout}"
        );
        assert!(!stdout.contains("restart app.socket"), "got: {stdout}");
        assert!(!stdout.contains("unit web.service"), "got: {stdout}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn declared_tcp_ports_are_checked_with_ss() {
        let mut host = host();
        host.nod_config.health_checks.tcp_ports = Some(vec![443, 8080]);

        let command = extra_probe_command(&host, &[]).expect("a declared port needs a probe");

        assert!(command.contains("ss -ltnH"));
        assert!(command.contains("sport = :443"));
        assert!(command.contains("sport = :8080"));
        assert!(command.contains("ss is not installed"));
    }

    /// Proves the port probe against a `ss` double: one port answers, one does
    /// not, and only the silent one becomes a problem line.
    #[tokio::test]
    async fn the_port_probe_reports_only_the_closed_port() {
        let dir = shim(
            "ss",
            r#"#!/bin/sh
case "$*" in
  *":443"*) echo 'LISTEN 0 4096 0.0.0.0:443 0.0.0.0:*' ;;
  *) : ;;
esac
"#,
        );

        let mut host = host();
        host.nod_config.health_checks.tcp_ports = Some(vec![443, 8080]);
        let command = extra_probe_command(&host, &[]).expect("declared ports need a probe");

        let output = tokio::process::Command::new("sh")
            .args(["-c", &command])
            .env("PATH", path_with(&dir))
            .output()
            .await
            .expect("the probe shell should run");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("tcp port 8080: not listening"),
            "got: {stdout}"
        );
        assert!(!stdout.contains("tcp port 443"), "got: {stdout}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_probes_use_curl_and_the_expected_status() {
        let mut host = host();
        host.nod_config.health_checks.http_probes = Some(vec![HttpProbeConfig {
            url: Some("https://example.org/health".to_string()),
            expected_status: Some(204),
            timeout_secs: Some(3),
        }]);

        let command = extra_probe_command(&host, &[]).expect("an http probe needs a probe");
        assert!(command.contains("curl"));
        assert!(command.contains("https://example.org/health"));
        assert!(command.contains("204"));
    }

    #[test]
    fn custom_probes_run_under_a_timeout() {
        let mut host = host();
        host.nod_config.health_checks.custom_probes = Some(vec![CustomProbeConfig {
            name: Some("socket".to_string()),
            command: Some("test -S /run/app.sock".to_string()),
            timeout_secs: Some(2),
        }]);

        let command = extra_probe_command(&host, &[]).expect("a custom probe needs a probe");
        assert!(command.contains("timeout 2"));
        assert!(command.contains("test -S /run/app.sock"));
        assert!(command.contains("socket"));
    }

    #[test]
    fn extra_problems_are_included_in_the_snapshot() {
        let mut host = host();
        host.nod_config.health_checks.systemd.required_units =
            Some(vec!["web.service".to_string()]);

        let snapshot = snapshot(&host, "", "", "unit web.service: inactive (failed)\n");
        assert_eq!(
            snapshot.extra_problems,
            vec!["unit web.service: inactive (failed)".to_string()]
        );
        assert!(!snapshot.is_clean());
    }

    #[test]
    fn a_restart_counter_line_is_parsed_into_the_snapshot() {
        let snapshot = snapshot(&host(), "", "", "restart web.service 2\n");
        assert_eq!(snapshot.restarts.get("web.service"), Some(&2));
        assert!(snapshot.extra_problems.is_empty());
    }

    #[test]
    fn max_restarts_defaults_to_zero_and_honours_configuration() {
        let host = host();
        assert_eq!(max_restarts(&host), 0);

        let mut configured = host;
        configured.nod_config.health_checks.systemd.max_restarts = Some(2);
        assert_eq!(max_restarts(&configured), 2);
    }
}
