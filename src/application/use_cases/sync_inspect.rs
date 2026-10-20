//! Shared host inspection for reconciliation and status (ADR-028).
//!
//! One sweep answers three questions per host - did it answer, what closure
//! does it declare, and what does it run - so the reconciler, the status
//! command and `--plan` cannot drift apart in what they consider drift.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Whether a host answered the reachability probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// The host answered.
    Reachable,
    /// The host did not answer; the optional detail explains why the probe
    /// itself failed (as opposed to answering "not reachable").
    Unreachable { detail: Option<String> },
}

/// One host's observed state during a sweep.
#[derive(Debug, Clone)]
pub struct HostInspection {
    /// The inspected host.
    pub host: HostEntity,
    /// Reachability verdict.
    pub reachability: Reachability,
    /// The built toplevel, when the host answered.
    pub target: Option<PathBuf>,
    /// The live closure, when the host answered.
    pub current: Option<PathBuf>,
}

impl HostInspection {
    /// `true` when the host did not answer.
    pub fn is_unreachable(&self) -> bool {
        matches!(self.reachability, Reachability::Unreachable { .. })
    }

    /// `true` when the built closure differs from the live one.
    ///
    /// An unreachable host is never "drifted": it is unknown, which is a
    /// different statement and must stay distinguishable in reports.
    pub fn drifted(&self) -> bool {
        match (&self.target, &self.current) {
            (Some(target), Some(current)) => target != current,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

/// Inspects every host: probe, build the declared toplevel, read the live one.
///
/// The sweep runs concurrently under `concurrency` and returns results sorted by
/// host name, so output order never depends on completion order. A build
/// failure aborts the sweep: a partial answer would make the caller report
/// something it did not observe.
pub async fn inspect_hosts(
    ctx: &Arc<AppContext>,
    flake_path: &Path,
    hosts: Vec<HostEntity>,
    concurrency: usize,
    verbose: bool,
) -> Result<Vec<HostInspection>, NodError> {
    let evaluator = ctx.evaluator();
    let permits = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
    let mut tasks = tokio::task::JoinSet::new();

    for host in hosts {
        let evaluator = evaluator.clone();
        let ctx = ctx.clone();
        let permits = permits.clone();
        let flake = flake_path.to_path_buf();
        tasks.spawn(async move {
            let _permit = permits.acquire().await.ok();
            let deployer = ctx.deployer_for(&host);

            let reachability = match deployer.check_reachability(&host).await {
                Ok(true) => Reachability::Reachable,
                Ok(false) => Reachability::Unreachable { detail: None },
                Err(error) => Reachability::Unreachable {
                    detail: Some(error.to_string()),
                },
            };

            if matches!(reachability, Reachability::Unreachable { .. }) {
                return Ok(HostInspection {
                    host,
                    reachability,
                    target: None,
                    current: None,
                });
            }

            let target = evaluator
                .build_toplevel(&flake, &host.name, host.closure_attr.clone(), None, verbose)
                .await?;
            let profile = ctx.resolved_profile(&host).await?;
            let current = match deployer.current_closure(&host, &profile).await {
                Ok(current) => current,
                Err(error) => {
                    // The host answered the probe but its live closure could not
                    // be read. That is "unknown", not "drifted": reporting a
                    // permission or transport failure as drift would make the
                    // status lie about the fleet.
                    return Ok(HostInspection {
                        host,
                        reachability: Reachability::Unreachable {
                            detail: Some(error.to_string()),
                        },
                        target: None,
                        current: None,
                    });
                }
            };

            Ok(HostInspection {
                host,
                reachability,
                target: Some(target),
                current,
            })
        });
    }

    let mut inspections = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let inspection = joined
            .map_err(|error| NodError::internal(format!("inspection task failed: {error}")))??;
        inspections.push(inspection);
    }
    inspections.sort_by(|left, right| left.host.name.cmp(&right.host.name));
    Ok(inspections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn inspection(target: Option<&str>, current: Option<&str>) -> HostInspection {
        HostInspection {
            host: HostEntity::new("atlas", "10.0.0.8", false),
            reachability: Reachability::Reachable,
            target: target.map(PathBuf::from),
            current: current.map(PathBuf::from),
        }
    }

    #[test]
    fn drift_compares_target_with_current() {
        assert!(!inspection(Some("/a"), Some("/a")).drifted());
        assert!(inspection(Some("/b"), Some("/a")).drifted());
    }

    #[test]
    fn a_missing_live_closure_counts_as_drift() {
        assert!(inspection(Some("/b"), None).drifted());
    }

    #[test]
    fn unreachable_is_not_drift() {
        let unreachable = HostInspection {
            host: HostEntity::new("atlas", "10.0.0.8", false),
            reachability: Reachability::Unreachable { detail: None },
            target: None,
            current: None,
        };
        assert!(unreachable.is_unreachable());
        assert!(!unreachable.drifted());
    }
}
