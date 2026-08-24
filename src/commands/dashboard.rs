//! `nod dashboard` command: launch the interactive Ratatui TUI dashboard.
//!
//! Boots the same `AppContext` the other commands use, then hands off to the
//! presentation layer, which owns the terminal session.

use std::path::Path;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::ui::run_dashboard;

pub async fn execute(ctx: AppContext, flake_path: &Path) -> Result<(), NodError> {
    run_dashboard(ctx, Some(flake_path.to_path_buf())).await?;
    Ok(())
}
