//! `nod build` command: evaluate and build a host or fleet's toplevel closure
//! (`system.build.toplevel`), optionally creating the `--out-link` symlink,
//! WITHOUT transferring or activating. Delegates to `DeployFleetUseCase`
//! (ADR-005, ADR-006 lifecycle commands).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::TargetSelection;
use crate::application::use_cases::deploy_fleet::{DeployFleetUseCase, FleetSummary};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::host::{BuilderHost, HostEntity};
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    target: Option<&str>,
    flake_path: Option<&Path>,
    verbose: bool,
    quiet: bool,
    cli_overrides: CliOverrides,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    out_link: Option<&Path>,
    builder: Option<&str>,
    concurrency: Option<usize>,
) -> Result<(), NodError> {
    let flake_path = flake_path.unwrap_or_else(|| Path::new("."));
    let concurrency = concurrency.unwrap_or(4);
    if concurrency == 0 {
        return Err(NodError::config("--concurrency must be at least 1"));
    }

    let config_store = TomlConfigStore::new(flake_path, cli_overrides)?;
    let ctx = AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_config_store(Arc::new(config_store));

    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    let mut options = DeploymentOptions {
        dry_run: false,
        concurrency,
        strategy: RolloutStrategy::Batch,
        batch_size: 0,
        fail_fast: false,
        auto_rollback: false,
        action: DeploymentAction::Build,
        verbose,
        out_link: out_link.map(|p| p.to_path_buf()),
        builder: None,
    };

    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let (effective_target, effective_all) =
        if !all && target.is_none() && tag.is_none() && role.is_none() {
            (Some("local"), false)
        } else {
            (target, all)
        };
    let targets = TargetSelection::select(
        hosts.clone(),
        effective_target,
        tag,
        role,
        effective_all,
        &local_hostname,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            effective_target.unwrap_or("all"),
            tag,
            role,
        ));
    }

    // Builder resolution cascade: CLI `--builder` wins; otherwise the flake
    // `config.nod.build.buildHost` default applies only when the build run
    // resolves to exactly one target carrying a build host.
    let builder_selector = resolve_builder_selector(builder, &targets);

    let mut builder_profile: Option<BuilderHost> = None;
    if let Some(sel) = builder_selector {
        let builder_host = TargetSelection::select_exact_one(
            hosts.clone(),
            Some(sel),
            None,
            None,
            false,
            &local_hostname,
        )?;
        if builder_host.is_local {
            // Identity-to-local: the builder selector resolves to this machine,
            // so there is nothing SSH-specific to forward.
            builder_profile = None;
        } else {
            // Resolve the connection profile for the chosen builder host.
            let profile = store.resolve(&builder_host).await?;
            builder_profile = Some(BuilderHost {
                target_host: builder_host.target_host.clone(),
                profile,
            });
        }
    }

    options.builder = builder_profile;

    let mut staged = Vec::<HostEntity>::with_capacity(targets.len());
    for host in targets {
        staged.push(store.apply_to(host).await?);
    }

    let use_case = DeployFleetUseCase::new(Arc::new(ctx));
    let summary = use_case.execute(staged, options, flake_path).await?;

    if !quiet {
        render_summary(&summary, verbose);
    }

    Ok(())
}

/// Renders the fleet result for the operator.
fn render_summary(summary: &FleetSummary, _verbose: bool) {
    for outcome in summary.outcomes.clone() {
        let label = format!("[{}]", outcome.state.to_str());
        let colored = if outcome.ok {
            label.green()
        } else {
            label.red()
        };
        println!("  {} {}", outcome.host_name.bold(), colored);
        if let Some(failure) = outcome.failure.as_deref() {
            println!("      {}", failure.red().dimmed());
        }
    }
    println!(
        "\n  {}",
        format!(
            "{} succeeded, {} rolled back, {} failed (aborted: {})",
            summary.succeeded(),
            summary.rolled_back(),
            summary.failed(),
            summary.aborted
        )
        .dimmed()
    );
}

/// External builder precedence: a CLI `--builder` wins outright; otherwise
/// the flake `config.nod.build.buildHost` default applies only when the build
/// run resolves to exactly one target carrying a build host.
fn resolve_builder_selector<'a>(
    builder: Option<&'a str>,
    targets: &'a [HostEntity],
) -> Option<&'a str> {
    builder.or_else(|| {
        if targets.len() == 1 {
            targets[0].nod_config.build.build_host.as_deref()
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostEntity;

    fn host_with_build_host(name: String, build_host: Option<String>) -> HostEntity {
        let mut host = HostEntity::new(name.clone(), name, false);
        host.nod_config.build.build_host = build_host;
        host
    }

    #[test]
    fn builder_selector_cli_override_wins_over_flake_default() {
        let targets = vec![host_with_build_host(
            "jello".to_string(),
            Some("flakey".to_string()),
        )];
        // CLI `--builder` wins outright, even with a flake default present.
        assert_eq!(
            resolve_builder_selector(Some("argy"), &targets),
            Some("argy")
        );
    }

    #[test]
    fn builder_selector_single_target_uses_flake_default_without_cli() {
        let targets = vec![host_with_build_host(
            "jello".to_string(),
            Some("flakey".to_string()),
        )];
        assert_eq!(resolve_builder_selector(None, &targets), Some("flakey"));
    }

    #[test]
    fn builder_selector_single_target_without_default_resolves_none() {
        let targets = vec![host_with_build_host("jello".to_string(), None)];
        assert_eq!(resolve_builder_selector(None, &targets), None);
    }

    #[test]
    fn builder_selector_multiple_targets_skip_flake_default() {
        let targets = vec![
            host_with_build_host("jello".to_string(), Some("flakey".to_string())),
            host_with_build_host("atlas".to_string(), Some("atlasy".to_string())),
        ];
        // With more than one target there is no unambiguous flake default.
        assert_eq!(resolve_builder_selector(None, &targets), None);
    }
}
