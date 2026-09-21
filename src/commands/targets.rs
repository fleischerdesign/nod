//! Target selection as the commands use it: filter by requirement, then name what the
//! requirement excluded.
//!
//! The filtering is an application decision (`selection::resolve_targets`), naming what
//! was excluded is a presentation one - and this is the *only* place a command selects
//! targets, so "filter and stay silent" is not a shape the code offers (ADR-026).
//!
//! A skip is never silent because the two are indistinguishable otherwise: an inventory
//! target a lifecycle command ignored and a fleet member the command lost produce the
//! same shorter list. The operator gets the names and the reason instead.

use colored::Colorize;

use crate::application::selection::{
    resolve_targets, skipped_by, TargetAxes, TargetRequirement, TargetSelection,
};
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Selects the targets of `action`, naming every discovered target it cannot act on.
///
/// `action` is the phrase the operator sees, e.g. `"switch/test/boot"`.
pub fn select(
    hosts: Vec<HostEntity>,
    local_hostname: &str,
    axes: TargetAxes<'_>,
    requirement: TargetRequirement,
    action: &str,
) -> Vec<HostEntity> {
    report_skipped(&hosts, requirement, action);
    resolve_targets(hosts, local_hostname, axes, requirement)
}

/// Prints the targets a requirement excludes, once, before the command acts.
pub fn report_skipped(discovered: &[HostEntity], requirement: TargetRequirement, action: &str) {
    let skipped = skipped_by(discovered, requirement);
    if skipped.is_empty() {
        return;
    }
    let names: Vec<&str> = skipped.iter().map(|host| host.name.as_str()).collect();
    println!(
        "{}",
        format!(
            "{action}: {} target(s) {} - skipped: {}",
            names.len(),
            requirement.skip_reason(),
            names.join(", ")
        )
        .dimmed()
    );
}

/// Selects the one target of `action`, naming every discovered target it cannot act on.
///
/// An exact-one command states its requirement like every other command: `ssh` needs a
/// shell, `rollback` a closure, a build host a shell. A target that exists but cannot
/// satisfy it is refused with its reason instead of being silently replaced by a local run.
pub fn select_one(
    hosts: Vec<HostEntity>,
    local_hostname: &str,
    axes: TargetAxes<'_>,
    requirement: TargetRequirement,
    action: &str,
) -> Result<HostEntity, NodError> {
    report_skipped(&hosts, requirement, action);
    TargetSelection::select_exact_one(hosts, axes, local_hostname, requirement)
}

/// Refuses a target that cannot satisfy a requirement, for the commands that select one
/// host directly (`ssh`).
///
/// Returning `None` for an eligible target and a ready error otherwise keeps the message
/// in one place; the caller only decides what to do with it.
pub fn refuse_ineligible(host: &HostEntity, requirement: TargetRequirement) -> Option<String> {
    (!requirement.admits(host)).then(|| {
        let reason = match requirement {
            TargetRequirement::Shell => {
                "is activated through a device API and has no shell to open"
            }
            TargetRequirement::Closure => "declares no closure (an inventory target)",
            TargetRequirement::RunningSystem => {
                "is activated through a device API and has no running system to compare"
            }
            TargetRequirement::Reachability => "is unreachable",
        };
        format!("target '{}' {reason}", host.name)
    })
}
