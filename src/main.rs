use anyhow::Result;
use clap::Parser;
use nod::config::options::{Cli, Commands, SshArgs, TargetArgs};
use nod::domain::config::CliOverrides;
use nod::infrastructure::storage::json_audit_store::JsonAuditStore;
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    nod::telemetry::init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Switch {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            ssh_args:
                SshArgs {
                    user,
                    port,
                    identity_file,
                },
            flake,
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
                identity_file,
            };
            let ctx = nod::commands::wiring::production(Path::new(&flake), overrides)?;
            nod::commands::switch::execute(
                ctx,
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                cli.quiet,
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
            )
            .await?;
        }
        Commands::Test {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            ssh_args:
                SshArgs {
                    user,
                    port,
                    identity_file,
                },
            flake,
            concurrency,
            strategy,
            batch_size,
            fail_fast,
            auto_rollback,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file,
            };
            let flake_path = flake.as_deref().unwrap_or_else(|| Path::new("."));
            let ctx = nod::commands::wiring::production(flake_path, overrides)?;
            nod::commands::test::execute(
                ctx,
                target.as_deref(),
                Some(flake_path),
                cli.verbose,
                cli.quiet,
                tag.as_deref(),
                role.as_deref(),
                all,
                concurrency,
                strategy.as_deref(),
                batch_size,
                fail_fast,
                auto_rollback,
            )
            .await?;
        }
        Commands::Boot {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            ssh_args:
                SshArgs {
                    user,
                    port,
                    identity_file,
                },
            flake,
            concurrency,
            strategy,
            batch_size,
            fail_fast,
            auto_rollback,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file,
            };
            let flake_path = flake.as_deref().unwrap_or_else(|| Path::new("."));
            let ctx = nod::commands::wiring::production(flake_path, overrides)?;
            nod::commands::boot::execute(
                ctx,
                target.as_deref(),
                Some(flake_path),
                cli.verbose,
                cli.quiet,
                tag.as_deref(),
                role.as_deref(),
                all,
                concurrency,
                strategy.as_deref(),
                batch_size,
                fail_fast,
                auto_rollback,
            )
            .await?;
        }
        Commands::Build {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            flake,
            builder,
            out_link,
            concurrency,
        } => {
            let flake_path = flake.as_deref().unwrap_or_else(|| Path::new("."));
            let ctx = nod::commands::wiring::production(flake_path, CliOverrides::default())?;
            nod::commands::build::execute(
                ctx,
                target.as_deref(),
                Some(flake_path),
                cli.verbose,
                cli.quiet,
                tag.as_deref(),
                role.as_deref(),
                all,
                out_link.as_deref(),
                builder.as_deref(),
                concurrency,
            )
            .await?;
        }
        Commands::Check { flake } => {
            nod::commands::check::execute(Path::new(&flake)).await?;
        }
        Commands::Status {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            flake,
        } => {
            let ctx =
                nod::commands::wiring::production(Path::new(&flake), CliOverrides::default())?;
            nod::commands::status::execute(
                ctx,
                Path::new(&flake),
                cli.verbose,
                tag.as_deref(),
                role.as_deref(),
                target.as_deref(),
                all,
            )
            .await?;
        }
        Commands::Diff {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            ssh_args:
                SshArgs {
                    user,
                    port,
                    identity_file,
                },
            flake,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file,
            };
            let ctx = nod::commands::wiring::production(Path::new(&flake), overrides)?;
            nod::commands::diff::execute(
                ctx,
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                tag.as_deref(),
                role.as_deref(),
                all,
            )
            .await?;
        }
        Commands::Plan {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            ssh_args:
                SshArgs {
                    user,
                    port,
                    identity_file,
                },
            flake,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file,
            };
            let ctx = nod::commands::wiring::production(Path::new(&flake), overrides)?;
            nod::commands::plan::execute(
                ctx,
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                tag.as_deref(),
                role.as_deref(),
                all,
            )
            .await?;
        }
        Commands::Rollback {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            flake,
            user,
            port,
        } => {
            let overrides = CliOverrides {
                user,
                port,
                identity_file: None,
            };
            let ctx = nod::commands::wiring::production(Path::new(&flake), overrides)?;
            nod::commands::rollback::execute(
                ctx,
                target.as_deref(),
                Path::new(&flake),
                cli.verbose,
                tag.as_deref(),
                role.as_deref(),
                all,
            )
            .await?;
        }
        Commands::Drift {
            target_args:
                TargetArgs {
                    target,
                    tag,
                    role,
                    all,
                },
            json,
        } => {
            let ctx = nod::commands::wiring::production(Path::new("."), CliOverrides::default())?;
            nod::commands::drift::execute(
                ctx,
                Path::new("."),
                cli.verbose,
                target.as_deref(),
                tag.as_deref(),
                role.as_deref(),
                all,
                json,
            )
            .await?;
        }
        Commands::Audit {
            target,
            limit,
            json,
        } => {
            // `audit` binds the audit store explicitly on the production graph.
            let ctx = nod::commands::wiring::production(Path::new("."), CliOverrides::default())?
                .with_audit_store(Arc::new(JsonAuditStore::new()));
            nod::commands::audit::execute(ctx, target.as_deref(), limit, json).await?;
        }
        Commands::Ssh {
            target,
            tag,
            role,
            sudo,
            command,
        } => {
            // AC3: `ssh` receives a production context with a flake path (`.`
            // by default), so resolved identity/proxy/port from the config
            // store are honoured instead of the primitive fallback.
            let ctx = nod::commands::wiring::production(Path::new("."), CliOverrides::default())?;
            nod::commands::ssh::execute(
                ctx,
                Some(Path::new(".")),
                target.as_deref(),
                tag.as_deref(),
                role.as_deref(),
                sudo,
                &command,
            )
            .await?;
        }
        Commands::Exec {
            target,
            flake,
            tag,
            role,
            all,
            sudo,
            concurrency,
            fail_fast,
            json,
            command,
        } => {
            let flake_path = flake.as_deref().unwrap_or_else(|| Path::new("."));
            let ctx = nod::commands::wiring::production(flake_path, CliOverrides::default())?;
            nod::commands::exec::execute(
                ctx,
                Some(flake_path),
                target.as_deref(),
                tag.as_deref(),
                role.as_deref(),
                all,
                sudo,
                concurrency,
                fail_fast,
                json,
                &command,
            )
            .await?;
        }
        Commands::Dashboard { flake } => {
            let ctx =
                nod::commands::wiring::production(Path::new(&flake), CliOverrides::default())?;
            nod::commands::dashboard::execute(Arc::new(ctx), Path::new(&flake)).await?;
        }
    }

    Ok(())
}
