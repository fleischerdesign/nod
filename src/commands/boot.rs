//! `nod boot` command: run `switch-to-configuration boot` on one host or a
//! fleet. Delegates rollout to `DeployFleetUseCase` (ADR-005, ADR-006, ADR-010).

use std::path::Path;

use crate::application::context::AppContext;
use crate::commands::lifecycle::{execute_lifecycle, LifecycleParams};
use crate::domain::errors::NodError;
use crate::domain::plan::DeploymentAction;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    flake_path: Option<&Path>,
    verbose: bool,
    quiet: bool,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    concurrency: Option<usize>,
    strategy: Option<&str>,
    batch_size: Option<usize>,
    fail_fast: bool,
    auto_rollback: bool,
) -> Result<(), NodError> {
    let flake_path = flake_path.unwrap_or_else(|| Path::new("."));
    let concurrency = concurrency.unwrap_or(4);
    let strategy = strategy.unwrap_or("batch");
    let batch_size = batch_size.unwrap_or(0);

    execute_lifecycle(
        ctx,
        flake_path,
        DeploymentAction::Boot,
        LifecycleParams {
            target,
            tag,
            role,
            all,
            dry_run: false,
            concurrency,
            strategy,
            batch_size,
            fail_fast,
            auto_rollback,
            verbose,
            quiet,
        },
    )
    .await
}
