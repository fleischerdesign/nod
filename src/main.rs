use anyhow::Result;
use clap::Parser;
use nod::config::options::{Cli, Commands, TargetArgs};
use nod::domain::config::CliOverrides;
use nod::infrastructure::config::toml_config::effective_flake;
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
            ssh_args,
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
            let overrides = ssh_args.into();
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, overrides)?;
            nod::commands::switch::execute(
                ctx,
                target.as_deref(),
                &flake_path,
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
            ssh_args,
            flake,
            concurrency,
            strategy,
            batch_size,
            fail_fast,
            auto_rollback,
        } => {
            let overrides = ssh_args.into();
            let flake_path = effective_flake(
                flake.as_deref().unwrap_or_else(|| Path::new(".")),
                Path::new("."),
            )?;
            let ctx = nod::commands::wiring::production(&flake_path, overrides)?;
            nod::commands::test::execute(
                ctx,
                target.as_deref(),
                Some(&flake_path),
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
            ssh_args,
            flake,
            concurrency,
            strategy,
            batch_size,
            fail_fast,
            auto_rollback,
        } => {
            let overrides = ssh_args.into();
            let flake_path = effective_flake(
                flake.as_deref().unwrap_or_else(|| Path::new(".")),
                Path::new("."),
            )?;
            let ctx = nod::commands::wiring::production(&flake_path, overrides)?;
            nod::commands::boot::execute(
                ctx,
                target.as_deref(),
                Some(&flake_path),
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
            let flake_path = effective_flake(
                flake.as_deref().unwrap_or_else(|| Path::new(".")),
                Path::new("."),
            )?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::build::execute(
                ctx,
                target.as_deref(),
                Some(&flake_path),
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
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            nod::commands::check::execute(&flake_path).await?;
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
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::status::execute(
                ctx,
                &flake_path,
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
            ssh_args,
            flake,
        } => {
            let overrides = ssh_args.into();
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, overrides)?;
            nod::commands::diff::execute(
                ctx,
                target.as_deref(),
                &flake_path,
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
            ssh_args,
            flake,
        } => {
            let overrides = ssh_args.into();
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, overrides)?;
            nod::commands::plan::execute(
                ctx,
                target.as_deref(),
                &flake_path,
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
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, overrides)?;
            nod::commands::rollback::execute(
                ctx,
                target.as_deref(),
                &flake_path,
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
            flake,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::drift::execute(
                ctx,
                &flake_path,
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
            flake,
            target,
            limit,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::audit::execute(ctx, target.as_deref(), limit, json).await?;
        }
        Commands::Ssh {
            flake,
            target,
            tag,
            role,
            sudo,
            command,
        } => {
            // AC3: `ssh` receives a production context with the resolved flake
            // path (explicit `--flake`, else `[defaults].flake`, else cwd), so
            // resolved identity/proxy/port from the config store are honoured
            // instead of the primitive fallback.
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::ssh::execute(
                ctx,
                Some(&flake_path),
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
            let flake_path = effective_flake(
                flake.as_deref().unwrap_or_else(|| Path::new(".")),
                Path::new("."),
            )?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::exec::execute(
                ctx,
                Some(&flake_path),
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
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::dashboard::execute(Arc::new(ctx), &flake_path).await?;
        }
        Commands::Inputs { flake, json } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::inputs::execute(ctx, &flake_path, json).await?;
        }
        Commands::Metadata { flake, json } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::metadata::execute(ctx, &flake_path, json).await?;
        }
        Commands::Update {
            flake,
            inputs,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::update::execute(ctx, &flake_path, inputs, json).await?;
        }
        Commands::Generations {
            target_args,
            flake,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::generations::execute(ctx, &flake_path, &target_args, cli.verbose, json)
                .await?;
        }
        Commands::Gc {
            target_args,
            flake,
            older_than,
            keep,
            dry_run,
            concurrency,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let options = nod::domain::generation::GcOptions {
                older_than,
                keep,
                dry_run,
                concurrency,
            };
            nod::commands::gc::execute(ctx, &flake_path, &target_args, options, cli.verbose, json)
                .await?;
        }
        Commands::Copy {
            target_args,
            flake,
            to,
            from,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let options = nod::domain::generation::CopyOptions { to, from };
            nod::commands::copy::execute(
                ctx,
                &flake_path,
                &target_args,
                options,
                cli.verbose,
                json,
            )
            .await?;
        }
    }

    Ok(())
}
