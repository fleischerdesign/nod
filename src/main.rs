mod application;
mod commands;
mod config;
mod domain;
mod infrastructure;
mod telemetry;
mod ui;

use anyhow::Result;
use clap::Parser;
use config::options::{Cli, Commands};
use domain::config::CliOverrides;
use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Switch {
            target,
            flake,
            tag,
            role,
            user,
            port,
            identity_file,
            dry_run,
            concurrency,
            strategy,
            batch_size,
            fail_fast,
            auto_rollback,
            on_error,
            action,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: identity_file.map(PathBuf::from),
            };
            commands::switch::execute(
                &target,
                Path::new(&flake),
                cli.verbose,
                cli.quiet,
                overrides,
                tag.as_deref(),
                role.as_deref(),
                dry_run,
                concurrency,
                &strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                on_error.as_deref(),
                &action,
            ).await?;
        }
        Commands::Check { flake } => {
            commands::check::execute(Path::new(&flake)).await?;
        }
        Commands::Status { flake, tag, role } => {
            commands::status::execute(
                Path::new(&flake),
                cli.verbose,
                tag.as_deref(),
                role.as_deref(),
            ).await?;
        }
        Commands::Diff {
            target,
            flake,
            tag,
            role,
            user,
            port,
            identity_file,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: identity_file.map(PathBuf::from),
            };
            commands::diff::execute(
                &target,
                Path::new(&flake),
                cli.verbose,
                overrides,
                tag.as_deref(),
                role.as_deref(),
            ).await?;
        }
        Commands::Plan {
            target,
            flake,
            tag,
            role,
            user,
            port,
            identity_file,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: identity_file.map(PathBuf::from),
            };
            commands::plan::execute(
                &target,
                Path::new(&flake),
                cli.verbose,
                overrides,
                tag.as_deref(),
                role.as_deref(),
            ).await?;
        }
        Commands::Rollback { target, flake, user, port, timeout: _, target_opt } => {
            let effective = target_opt.unwrap_or(target);
            let overrides = CliOverrides {
                user,
                port,
                identity_file: None,
            };
            commands::rollback::execute(
                &effective,
                Path::new(&flake),
                cli.verbose,
                overrides,
            ).await?;
        }
        Commands::Drift { target, tag, json } => {
            commands::drift::execute(
                Path::new("."),
                cli.verbose,
                target.as_deref(),
                tag.as_deref(),
                json,
            ).await?;
        }
        Commands::History { target, limit, json } => {
            commands::history::execute(target.as_deref(), limit, json).await?;
        }
        Commands::Dashboard { flake } => {
            commands::dashboard::execute(Path::new(&flake)).await?;
        }
    }

    Ok(())
}
