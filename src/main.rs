mod commands;
mod config;
mod domain;
mod infrastructure;
mod telemetry;
mod ui;

use anyhow::Result;
use clap::Parser;
use config::options::{Cli, Commands};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Switch { target, flake } => {
            commands::switch::execute(&target, Path::new(&flake), cli.verbose, cli.quiet).await?;
        }
        Commands::Check { flake } => {
            commands::check::execute(Path::new(&flake)).await?;
        }
        Commands::Status { flake } => {
            commands::status::execute(Path::new(&flake), cli.verbose).await?;
        }
        Commands::Diff { target, flake } => {
            commands::diff::execute(&target, Path::new(&flake), cli.verbose).await?;
        }
        Commands::Rollback { target: _ } => {
            println!("Rollback engine reserved for generation profile rollback.");
        }
        Commands::Dashboard { flake: _ } => {
            println!("Ratatui interactive dashboard reserved for full-screen TUI.");
        }
    }

    Ok(())
}
