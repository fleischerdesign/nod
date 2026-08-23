//! `GeneratePlanUseCase`: build closures and evaluate diffs without activating
//! any host (ADR-003 planning stage). No switch command is ever dispatched.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentOptions, DeploymentPlan, TargetDiff, TargetPlan};

/// Builds a `DeploymentPlan` (and its diffs) while never activating systems.
pub struct GeneratePlanUseCase {
    ctx: Arc<AppContext>,
}

impl GeneratePlanUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Reuses the fleet planner for rollout wave ordering semantics.
    /// Builds every host's toplevel closure and returns a plan that has not
    /// activated anything.
    pub async fn plan(
        &self,
        hosts: Vec<HostEntity>,
        options: DeploymentOptions,
        flake_path: &Path,
    ) -> Result<DeploymentPlan, NodError> {
        let evaluator = self.ctx.evaluator();
        let mut targets = Vec::<TargetPlan>::with_capacity(hosts.len());
        for host in hosts {
            let closure = evaluator
                .build_toplevel(flake_path, &host.name, None, options.verbose)
                .await?;
            let current = if host.is_local {
                Some(PathBuf::from("/run/current-system"))
            } else {
                None
            };
            targets.push(TargetPlan {
                host_name: host.name,
                action: options.action.clone(),
                new_closure: Some(closure),
                current_closure: current,
            });
        }
        Ok(DeploymentPlan { targets, options })
    }

    /// Computes the diff view for a plan (preview). Targets without a staged
    /// closure are omitted from the diff records.
    pub fn diffs(&self, plan: DeploymentPlan) -> Vec<TargetDiff> {
        let mut out = Vec::<TargetDiff>::with_capacity(plan.targets.len());
        for target in plan.targets {
            if target.new_closure.is_none() {
                continue;
            }
            let current = target
                .current_closure
                .clone()
                .unwrap_or(PathBuf::from("/unknown"));
            let new_path = target.new_closure.clone().unwrap_or(PathBuf::from(""));
            out.push(TargetDiff {
                host_name: target.host_name,
                current_closure: target.current_closure.clone(),
                new_closure: new_path.clone(),
                changed: target.current_closure.is_none() || current != new_path,
            });
        }
        out
    }
}
