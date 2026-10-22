use anyhow::Result;
use clap::Parser;
use nod::config::options::{Cli, Commands, SyncArgs, TargetArgs};
use nod::domain::config::CliOverrides;
use nod::domain::watch::SyncOptions;
use nod::infrastructure::config::toml_config::effective_flake;
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    nod::telemetry::init_tracing();
    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::from(exit_status(&error))
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
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
        Commands::Eval {
            expr,
            target_args,
            flake,
            raw,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::eval::execute(
                ctx,
                &flake_path,
                &target_args,
                &expr,
                raw,
                cli.verbose,
                json,
            )
            .await?;
        }
        Commands::Repl { target, flake } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::repl::execute(ctx, &flake_path, target.as_deref(), cli.verbose).await?;
        }
        Commands::Info {
            target_args,
            flake,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            nod::commands::info::execute(ctx, &flake_path, &target_args, cli.verbose, json).await?;
        }
        Commands::Reboot {
            target_args,
            flake,
            strategy,
            batch_size,
            concurrency,
            no_wait,
            timeout,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let options = nod::application::use_cases::reboot_fleet::RebootOptions {
                strategy,
                batch_size,
                concurrency,
                wait: !no_wait,
                timeout_secs: timeout,
            };
            nod::commands::reboot::execute(ctx, &flake_path, &target_args, options, cli.verbose)
                .await?;
        }
        Commands::Provider { command } => match command {
            nod::config::options::ProviderCommands::List { dir } => {
                nod::commands::provider::list(&dir).await?
            }
            nod::config::options::ProviderCommands::Inspect { command } => {
                nod::commands::provider::inspect(&command).await?
            }
            nod::config::options::ProviderCommands::Schema { command } => {
                nod::commands::provider::schema(&command).await?
            }
            nod::config::options::ProviderCommands::Test {
                command,
                settings,
                write,
            } => nod::commands::provider::test(&command, settings.as_deref(), write).await?,
        },
        Commands::Reconcile {
            provider,
            command,
            settings,
            desired,
            apply,
            json,
            flake,
        } => {
            nod::commands::reconcile::execute(
                provider.as_deref(),
                command.as_deref(),
                settings.as_deref(),
                &desired,
                apply,
                json,
                &flake,
            )
            .await?;
        }
        Commands::Secrets { command } => match command {
            nod::config::options::SecretCommands::Check {
                target_args,
                flake,
                json,
            } => {
                let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
                let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
                nod::commands::secret::execute_check(
                    ctx,
                    &flake_path,
                    &target_args,
                    cli.verbose,
                    json,
                )
                .await?;
            }
            nod::config::options::SecretCommands::Rekey {
                target_args,
                flake,
                dry_run,
                no_backup,
                json,
            } => {
                let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
                let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
                let options = nod::domain::secret::RekeyOptions {
                    dry_run,
                    backup: !no_backup,
                };
                nod::commands::secret::execute_rekey(
                    ctx,
                    &flake_path,
                    &target_args,
                    options,
                    cli.verbose,
                    json,
                )
                .await?;
            }
        },
        Commands::Store { command } => match command {
            nod::config::options::StoreCommands::Optimize {
                target_args,
                flake,
                json,
            } => {
                let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
                let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
                nod::commands::store::execute_optimize(
                    ctx,
                    &flake_path,
                    &target_args,
                    cli.verbose,
                    json,
                )
                .await?;
            }
        },
        Commands::Cache { command } => match command {
            nod::config::options::CacheCommands::Push {
                target_args,
                flake,
                cache,
                concurrency,
                dry_run,
                json,
            } => {
                let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
                let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
                let options = nod::domain::cache::CachePushOptions {
                    cache_uri: cache,
                    dry_run,
                    concurrency,
                };
                nod::commands::cache::execute_push(
                    ctx,
                    &flake_path,
                    &target_args,
                    options,
                    cli.verbose,
                    json,
                )
                .await?;
            }
        },
        Commands::Graph { format, flake } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let parsed_format = format
                .parse::<nod::domain::topology::GraphFormat>()
                .map_err(nod::domain::errors::NodError::config)?;
            nod::commands::graph::execute(ctx, &flake_path, parsed_format, cli.verbose).await?;
        }
        Commands::Export { format, flake } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let parsed_format = format
                .parse::<nod::domain::topology::ExportFormat>()
                .map_err(nod::domain::errors::NodError::config)?;
            nod::commands::export::execute(ctx, &flake_path, parsed_format, cli.verbose).await?;
        }
        Commands::Bootstrap {
            target,
            ip,
            user,
            port,
            flake,
            no_disko,
            no_kexec,
            debug,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let options = nod::domain::provision::BootstrapOptions {
                target_ip: ip,
                ssh_user: user,
                ssh_port: port,
                disko: !no_disko,
                no_kexec,
                debug,
            };
            nod::commands::bootstrap::execute(
                ctx,
                &flake_path,
                &target,
                options,
                cli.verbose,
                json,
            )
            .await?;
        }
        Commands::Init {
            dir,
            template,
            name,
        } => {
            let parsed_template = template
                .parse::<nod::domain::provision::InitTemplate>()
                .map_err(nod::domain::errors::NodError::config)?;
            let path_buf = dir.map(std::path::PathBuf::from);
            nod::commands::init::execute(path_buf, parsed_template, name)?;
        }
        Commands::Iso {
            target,
            flake,
            format,
            json,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let options = nod::domain::provision::IsoOptions {
                target_format: format,
            };
            nod::commands::iso::execute(
                ctx,
                &flake_path,
                target.as_deref(),
                options,
                cli.verbose,
                json,
            )
            .await?;
        }
        Commands::Watch {
            target_args,
            flake,
            interval,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let options = nod::domain::watch::WatchOptions {
                poll_interval_secs: interval,
                debounce_ms: 500,
            };
            nod::commands::watch::execute(ctx, &flake_path, &target_args, options, cli.verbose)
                .await?;
        }
        Commands::Sync {
            flake,
            sync,
            once,
            command,
        } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            match command {
                Some(nod::config::options::SyncCommand::Status {
                    target_args,
                    drift,
                    json,
                }) => {
                    let options = SyncOptions {
                        remote: sync.remote.clone(),
                        branch: sync.branch.clone(),
                        state_dir: sync.state_dir.clone(),
                        json,
                        verbose: cli.verbose,
                        ..Default::default()
                    };
                    let hosts = if drift {
                        let discovered = ctx
                            .evaluator()
                            .discover_hosts_degraded(&flake_path, cli.verbose)
                            .await?;
                        let local_hostname = hostname::get()
                            .map(|host| host.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let scope = SyncOptions {
                            targets: target_args.target.into_iter().collect(),
                            all: target_args.all,
                            tag: target_args.tag,
                            role: target_args.role,
                            ..options.clone()
                        };
                        Some(nod::commands::sync::select_hosts(
                            discovered,
                            &scope,
                            &local_hostname,
                        )?)
                    } else {
                        None
                    };
                    nod::commands::sync_status::execute(ctx, &flake_path, options, hosts).await?;
                }
                None => {
                    let mut options = sync_options(sync, once);
                    options.verbose = cli.verbose;
                    nod::commands::sync::execute(ctx, &flake_path, options).await?;
                }
            }
        }
        Commands::Daemon { flake, sync } => {
            let flake_path = effective_flake(Path::new(&flake), Path::new("."))?;
            let ctx = nod::commands::wiring::production(&flake_path, CliOverrides::default())?;
            let mut options = sync_options(sync, false);
            options.verbose = cli.verbose;
            nod::commands::daemon::execute(ctx, &flake_path, options).await?;
        }
    }

    Ok(())
}

/// Maps a top-level error to the documented exit status (ADR-028).
///
/// 0 is success, 2 a configuration error, 3 a refusal before any host was
/// touched, and 1 everything that failed while acting.
fn exit_status(error: &anyhow::Error) -> u8 {
    use nod::domain::errors::NodError;
    match error.downcast_ref::<NodError>() {
        Some(NodError::Refused { .. }) => 3,
        Some(NodError::Config { .. }) => 2,
        _ => 1,
    }
}

/// Builds the reconciler options from the CLI surface.
fn sync_options(args: SyncArgs, once: bool) -> SyncOptions {
    SyncOptions {
        remote: args.remote,
        branch: args.branch,
        interval_secs: args.interval,
        gate_interval_secs: args.gate_interval,
        offline_backoff_secs: args.offline_backoff,
        offline_backoff_max_secs: args.offline_backoff_max,
        gone_after_secs: args.gone_after,
        jitter_percent: args.jitter,
        targets: args.targets,
        all: args.all,
        tag: args.tag,
        role: args.role,
        state_dir: args.state_dir,
        strategy: args.strategy,
        batch_size: args.batch_size,
        concurrency: args.concurrency,
        fail_fast: args.fail_fast,
        auto_rollback: !args.no_auto_rollback,
        allow_downgrade: args.allow_downgrade,
        require_signature: args.require_signature,
        last_hosts: args.last_hosts,
        notify: args.notify,
        json: args.json,
        verbose: false,
        dry_run: args.dry_run || args.plan,
        plan: args.plan,
        once,
    }
}
