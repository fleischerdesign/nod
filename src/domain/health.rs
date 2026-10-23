//! Post-activation health verdicts (ADR-029).
//!
//! Two observations, one decision. A snapshot is what the probes saw at one
//! moment; a verdict is what a deployment concludes by comparing the snapshot
//! it took before activating with the one it took after.
//!
//! The rules live here, in the domain, because they are policy and not I/O:
//! `probe` turns command output into a snapshot, the deployers run the
//! commands, and this module decides what the difference means.

use serde::{Deserialize, Serialize};

/// What the health probes observed at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Units systemd reports as failed.
    pub failed_units: Vec<String>,
    /// Units systemd is restarting on its own (`activating`/`auto-restart`).
    pub restarting_units: Vec<String>,
    /// Problems from declared units and probes, already rendered by the probe.
    ///
    /// Kept generic so a new probe kind does not change the snapshot type; the
    /// line names what failed (`unit web.service: inactive`, `http …`).
    #[serde(default)]
    pub extra_problems: Vec<String>,
}

impl HealthSnapshot {
    /// A snapshot with no problems at all.
    pub fn clean() -> Self {
        Self::default()
    }

    /// Every unit that is degraded, deduplicated and ordered.
    ///
    /// A unit can be both failed and restarting; it is one problem, not two.
    pub fn problems(&self) -> Vec<String> {
        let mut units: Vec<String> = self
            .failed_units
            .iter()
            .chain(self.restarting_units.iter())
            .chain(self.extra_problems.iter())
            .cloned()
            .collect();
        units.sort();
        units.dedup();
        units
    }

    /// Whether this snapshot shows no problem at all.
    pub fn is_clean(&self) -> bool {
        self.problems().is_empty()
    }
}

/// What a deployment concludes about one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum HealthVerdict {
    /// Every problem the snapshot shows is new, and there are none.
    Healthy,
    /// At least one problem appeared that the baseline did not have. This is
    /// the deployment's own damage and the only verdict that may roll back.
    Degraded {
        /// Units that regressed, ordered.
        units: Vec<String>,
    },
    /// Problems exist, but every one of them was already there before the
    /// activation. Reported, never punished.
    DegradedPreExisting {
        /// Units that were already degraded, ordered.
        units: Vec<String>,
    },
    /// The deployment could not be observed: checks are disabled, or the probe
    /// itself failed. Never reported as healthy, never rolled back.
    Unverified {
        /// Why no observation was possible.
        reason: String,
    },
    /// The new configuration needs a reboot before it takes effect.
    PendingReboot,
}

impl HealthVerdict {
    /// Whether the deployment caused the degradation.
    ///
    /// `DegradedPreExisting` deliberately does not count: rolling back a
    /// deployment for a fault it did not cause is how a rollout policy loses
    /// its credibility.
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Degraded { .. })
    }

    /// The units this verdict names, ordered.
    pub fn units(&self) -> &[String] {
        match self {
            Self::Healthy | Self::PendingReboot | Self::Unverified { .. } => &[],
            Self::Degraded { units } | Self::DegradedPreExisting { units } => units,
        }
    }

    /// Whether the fleet may treat this host as converged.
    ///
    /// Only `Healthy` counts. `PendingReboot` still owes a reboot and
    /// `Unverified` was never observed; calling either converged is the
    /// confusion this taxonomy exists to remove.
    pub fn is_converged(&self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// What one host's deployment produced, beyond "it did not error".
///
/// Activation errors stay errors; this is the verdict about the state the
/// activation left behind, which the caller needs in order to decide whether to
/// roll back, keep going, or report an unobserved host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentReport {
    /// What verification concluded.
    pub health: HealthVerdict,
}

impl DeploymentReport {
    /// A report that could not observe the host.
    pub fn unverified(reason: impl Into<String>) -> Self {
        Self {
            health: HealthVerdict::Unverified {
                reason: reason.into(),
            },
        }
    }

    /// A report that observed the host and found it healthy.
    pub fn healthy() -> Self {
        Self {
            health: HealthVerdict::Healthy,
        }
    }
}

/// Compares the post-activation observation with the pre-activation baseline.
///
/// The rules, in order:
///
/// 1. Nothing degraded now: `Healthy`, even when the baseline was worse. An
///    improvement is not a reason to complain.
/// 2. No usable baseline: `Degraded`. Not knowing what the host looked like is
///    not evidence that the deployment was harmless.
/// 3. Every current problem was already a problem: `DegradedPreExisting`.
/// 4. Anything else appeared: `Degraded`, naming only the units that regressed.
pub fn verdict_against(
    baseline: Option<&HealthSnapshot>,
    current: &HealthSnapshot,
) -> HealthVerdict {
    let current_problems = current.problems();
    if current_problems.is_empty() {
        return HealthVerdict::Healthy;
    }

    let Some(baseline) = baseline else {
        return HealthVerdict::Degraded {
            units: current_problems,
        };
    };

    let before = baseline.problems();
    let regressed: Vec<String> = current_problems
        .iter()
        .filter(|unit| !before.contains(unit))
        .cloned()
        .collect();

    if regressed.is_empty() {
        HealthVerdict::DegradedPreExisting {
            units: current_problems,
        }
    } else {
        HealthVerdict::Degraded { units: regressed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(failed: &[&str], restarting: &[&str]) -> HealthSnapshot {
        HealthSnapshot {
            failed_units: failed.iter().map(|unit| unit.to_string()).collect(),
            restarting_units: restarting.iter().map(|unit| unit.to_string()).collect(),
            extra_problems: Vec::new(),
        }
    }

    #[test]
    fn different_probe_kinds_for_the_same_unit_are_one_problem() {
        let snap = snapshot(&["crowdsec.service"], &["crowdsec.service"]);
        assert_eq!(snap.problems(), vec!["crowdsec.service".to_string()]);
    }

    #[test]
    fn a_clean_host_is_healthy_even_when_it_improved() {
        let baseline = snapshot(&["old.service"], &[]);
        let current = snapshot(&[], &[]);

        assert_eq!(
            verdict_against(Some(&baseline), &current),
            HealthVerdict::Healthy
        );
    }

    #[test]
    fn a_missing_baseline_never_counts_as_healthy() {
        let current = snapshot(&["new.service"], &[]);

        assert_eq!(
            verdict_against(None, &current),
            HealthVerdict::Degraded {
                units: vec!["new.service".to_string()]
            }
        );
    }

    #[test]
    fn a_preexisting_problem_is_reported_but_not_attributed() {
        let baseline = snapshot(&["old.service"], &[]);
        let current = snapshot(&["old.service"], &[]);

        let verdict = verdict_against(Some(&baseline), &current);
        assert_eq!(
            verdict,
            HealthVerdict::DegradedPreExisting {
                units: vec!["old.service".to_string()]
            }
        );
        assert!(!verdict.is_failure());
    }

    #[test]
    fn a_regression_names_only_the_units_it_caused() {
        let baseline = snapshot(&["old.service"], &[]);
        let current = snapshot(&["old.service", "new.service"], &["loop.service"]);

        assert_eq!(
            verdict_against(Some(&baseline), &current),
            HealthVerdict::Degraded {
                units: vec!["loop.service".to_string(), "new.service".to_string()]
            }
        );
    }

    #[test]
    fn a_restart_loop_that_started_after_the_deploy_is_a_failure() {
        let baseline = snapshot(&[], &[]);
        let current = snapshot(&[], &["crowdsec.service"]);

        let verdict = verdict_against(Some(&baseline), &current);
        assert!(verdict.is_failure());
        assert_eq!(verdict.units(), &["crowdsec.service".to_string()]);
    }

    #[test]
    fn a_preexisting_loop_stops_being_attributed_when_it_recovers_and_returns() {
        // The host was looping before, is clean at the baseline, and loops
        // again after: that is a regression, not history.
        let baseline = snapshot(&[], &[]);
        let current = snapshot(&[], &["crowdsec.service"]);

        assert!(verdict_against(Some(&baseline), &current).is_failure());
    }

    #[test]
    fn an_extra_problem_is_part_of_the_verdict() {
        let baseline = HealthSnapshot::clean();
        let current = HealthSnapshot {
            failed_units: Vec::new(),
            restarting_units: Vec::new(),
            extra_problems: vec!["unit web.service: inactive (failed)".to_string()],
        };

        let verdict = verdict_against(Some(&baseline), &current);
        assert!(verdict.is_failure());
        assert_eq!(
            verdict.units(),
            &["unit web.service: inactive (failed)".to_string()]
        );
    }
}
