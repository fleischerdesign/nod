//! Startup self-check for the host the daemon runs on (ADR-029).
//!
//! A host cannot verify the activation that stops the process performing it:
//! the deployer dies with the switch, and the old `unverified` entry would stay
//! forever. The daemon is the one process that can observe that host after the
//! restart, so it does exactly that once, before the first cycle.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::health::{verdict_against, HealthVerdict};
use crate::domain::host::HostEntity;
use crate::domain::sync::{now_secs, HostCondition, SyncState};
use crate::infrastructure::health::probe;

/// Probes the local host once and records the verdict in the reconciler state.
///
/// Returns `None` when the fleet has no host marked as `is_self`, which is the
/// normal case for a control plane that is not part of the fleet.
pub async fn execute(
    ctx: &AppContext,
    flake_path: &Path,
) -> Result<Option<HealthVerdict>, NodError> {
    let hosts = ctx
        .evaluator()
        .discover_hosts_degraded(flake_path, false)
        .await?;
    let Some(local) = hosts.iter().find(|host| host.is_self) else {
        return Ok(None);
    };

    let verdict = probe_local(local).await;

    let store = ctx.sync_state()?;
    let Some(mut state) = store.load().await? else {
        // The first cycle has not written a state document yet; there is
        // nothing to attach the verdict to, and inventing one would make the
        // daemon's first save a race.
        return Ok(Some(verdict));
    };

    state.record_health(&local.name, verdict.clone(), now_secs());
    mark_verified_when_all_healthy(&mut state, &hosts);
    store.save(&state).await?;

    Ok(Some(verdict))
}

/// Runs the two local probes with the host's timeout.
async fn probe_local(host: &HostEntity) -> HealthVerdict {
    if !probe::enabled(host) {
        return HealthVerdict::Unverified {
            reason: "health checks are disabled for this host".to_string(),
        };
    }

    let timeout = probe::timeout(host);
    let failed = run(
        "systemctl",
        &["--failed", "--no-legend", "--plain"],
        timeout,
    )
    .await;
    let restarting = run("sh", &["-c", probe::RESTARTING_UNITS_COMMAND], timeout).await;

    match (failed, restarting) {
        (Ok(failed), Ok(restarting)) => {
            let snapshot = probe::snapshot(host, &failed, &restarting);
            // There is no pre-activation baseline after a restart; an existing
            // problem is reported as this host's state, not attributed to a
            // deployment. `degraded` is the conservative answer.
            verdict_against(None, &snapshot)
        }
        (Err(reason), _) | (_, Err(reason)) => HealthVerdict::Unverified { reason },
    }
}

/// Runs one probe command, bounded by the host's timeout.
async fn run(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let output = tokio::time::timeout(timeout, Command::new(program).args(args).output()).await;
    match output {
        Ok(Ok(output)) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Ok(Err(error)) => Err(format!("cannot run the local health probe: {error}")),
        Err(_) => Err(format!(
            "the local health probe timed out after {timeout:?}"
        )),
    }
}

/// A fleet is verified at the applied revision only when every discovered host
/// is reachable and its last recorded verdict is `healthy`.
///
/// This is an aggregation of stored observations, not a new probe of each
/// host: the local one was just observed, the others were observed when they
/// were deployed.
fn mark_verified_when_all_healthy(state: &mut SyncState, hosts: &[HostEntity]) {
    let Some(applied) = state.last_applied_rev.clone() else {
        return;
    };

    let all_healthy = hosts.iter().all(|host| {
        state.hosts.get(&host.name).is_some_and(|memory| {
            matches!(memory.condition, HostCondition::Reachable)
                && matches!(memory.health, Some(HealthVerdict::Healthy))
        })
    });

    if all_healthy {
        state.last_verified_rev = Some(applied);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, is_self: bool) -> HostEntity {
        HostEntity::new(name, "10.0.0.1", is_self)
    }

    fn state_with(hosts: &[(&str, HealthVerdict)]) -> SyncState {
        let mut state = SyncState::new("repo", "main");
        state.last_applied_rev = Some("abc".to_string());
        for (name, verdict) in hosts {
            state.observe_reachable(name, 1);
            state.record_health(name, verdict.clone(), 1);
        }
        state
    }

    #[test]
    fn a_reachable_healthy_fleet_marks_the_revision_verified() {
        let hosts = vec![host("a", false), host("b", true)];
        let mut state = state_with(&[("a", HealthVerdict::Healthy), ("b", HealthVerdict::Healthy)]);

        mark_verified_when_all_healthy(&mut state, &hosts);
        assert_eq!(state.last_verified_rev.as_deref(), Some("abc"));
    }

    #[test]
    fn one_unverified_host_keeps_the_marker_behind() {
        let hosts = vec![host("a", false), host("b", true)];
        let mut state = state_with(&[
            ("a", HealthVerdict::Healthy),
            (
                "b",
                HealthVerdict::Unverified {
                    reason: "no probe".to_string(),
                },
            ),
        ]);

        mark_verified_when_all_healthy(&mut state, &hosts);
        assert_eq!(state.last_verified_rev, None);
    }

    #[test]
    fn an_offline_host_keeps_the_marker_behind() {
        let hosts = vec![host("a", false), host("b", true)];
        let mut state = state_with(&[("a", HealthVerdict::Healthy), ("b", HealthVerdict::Healthy)]);
        let policy = crate::domain::sync::RetryPolicy {
            base_secs: 60,
            max_secs: 1800,
            gone_after_secs: 86_400,
        };
        state.observe_offline("b", 2, &policy);

        mark_verified_when_all_healthy(&mut state, &hosts);
        assert_eq!(state.last_verified_rev, None);
    }

    #[test]
    fn a_host_without_memory_keeps_the_marker_behind() {
        let hosts = vec![host("a", true), host("b", false)];
        let mut state = state_with(&[("a", HealthVerdict::Healthy)]);

        mark_verified_when_all_healthy(&mut state, &hosts);
        assert_eq!(state.last_verified_rev, None);
    }

    #[test]
    fn no_applied_revision_means_no_verified_revision() {
        let hosts = vec![host("a", true)];
        let mut state = SyncState::new("repo", "main");
        state.observe_reachable("a", 1);
        state.record_health("a", HealthVerdict::Healthy, 1);

        mark_verified_when_all_healthy(&mut state, &hosts);
        assert_eq!(state.last_verified_rev, None);
    }
}
