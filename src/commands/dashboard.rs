//! `nod dashboard` command: launch the interactive Ratatui TUI dashboard.
//!
//! Boots the same `AppContext` the other commands use, then hands off to the
//! presentation layer, which owns the terminal session.

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;
use crate::ui::run_dashboard;

pub async fn execute(flake_path: &Path) -> Result<(), NodError> {
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    );
    run_dashboard(ctx, Some(flake_path.to_path_buf())).await?;
    Ok(())
}