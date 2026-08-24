//! `nod switch` command: rebuild and deploy Nix configurations for one host or
//! a fleet. Delegates rollout to `DeployFleetUseCase` (ADR-005, ADR-010).

use std::path::Path;

use crate::application::context::AppContext;
use crate::commands::lifecycle::{execute_lifecycle, LifecycleParams};
use crate::domain::errors::NodError;
use crate::domain::plan::DeploymentAction;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    quiet: bool,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    dry_run: bool,
    concurrency: usize,
    strategy: &str,
    batch_size: usize,
    fail_fast: bool,
    auto_rollback: bool,
    on_error: Option<&str>,
    action: &str,
) -> Result<(), NodError> {
    // `--continue-on-error` is the inverse of `--fail-fast`.
    let continue_on_error = on_error.map(|mode| mode == "continue").unwrap_or(false);
    let effective_fail_fast = !continue_on_error && fail_fast;
    let parsed_action = DeploymentAction::parse(action).unwrap_or(DeploymentAction::Switch);

    execute_lifecycle(
        ctx,
        flake_path,
        parsed_action,
        LifecycleParams {
            target,
            tag,
            role,
            all,
            dry_run,
            concurrency,
            strategy,
            batch_size,
            fail_fast: effective_fail_fast,
            auto_rollback,
            verbose,
            quiet,
        },
    )
    .await
}
