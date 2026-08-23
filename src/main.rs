use anyhow::Result;
use clap::Parser;
use nod::application::context::AppContext;
use nod::config::options::{Cli, Commands};
use nod::domain::config::CliOverrides;
use nod::infrastructure::deployment::local_deployer::LocalDeployer;
use nod::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use nod::infrastructure::nix::cli_evaluator::NixCliEvaluator;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    nod::telemetry::init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Switch {
            target,
            flake,
            tag,
            role,
            all,
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
            nod::commands::switch::execute(
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                cli.quiet,
                overrides,
                tag.as_deref(),
                role.as_deref(),
                all,
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
            nod::commands::check::execute(Path::new(&flake)).await?;
        }
        Commands::Status { target, flake, tag, role, all } => {
            nod::commands::status::execute(
                Path::new(&flake),
                cli.verbose,
                tag.as_deref(),
                role.as_deref(),
                target.as_deref(),
                all,
            ).await?;
        }
        Commands::Diff {
            target,
            flake,
            tag,
            role,
            all,
            user,
            port,
            identity_file,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: identity_file.map(PathBuf::from),
            };
            nod::commands::diff::execute(
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                overrides,
                tag.as_deref(),
                role.as_deref(),
                all,
            ).await?;
        }
        Commands::Plan {
            target,
            flake,
            tag,
            role,
            all,
            user,
            port,
            identity_file,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: identity_file.map(PathBuf::from),
            };
            nod::commands::plan::execute(
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                overrides,
                tag.as_deref(),
                role.as_deref(),
                all,
            ).await?;
        }
        Commands::Rollback { target, flake, tag, role, all, user, port, generation: _ } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: None,
            };
            nod::commands::rollback::execute(
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                overrides,
                tag.as_deref(),
                role.as_deref(),
                all,
            ).await?;
        }
        Commands::Drift { target, tag, role, all, json } => {
            nod::commands::drift::execute(
                Path::new("."),
                cli.verbose,
                target.as_deref(),
                tag.as_deref(),
                role.as_deref(),
                all,
                json,
            ).await?;
        }
        Commands::History { target, limit, json } => {
            nod::commands::history::execute(target.as_deref(), limit, json).await?;
        }
        Commands::Ssh { target, tag, role, sudo, command } => {
            let ctx = AppContext::new(
                Arc::new(NixCliEvaluator::new()),
                Arc::new(LocalDeployer::new()),
                Arc::new(SshCliDeployer::new()),
            );
            nod::commands::ssh::execute(
                ctx,
                None,
                target.as_deref(),
                tag.as_deref(),
                role.as_deref(),
                sudo,
                &command,
            ).await?;
        }
        Commands::Dashboard { flake } => {
            nod::commands::dashboard::execute(Path::new(&flake)).await?;
        }
    }

    Ok(())
}