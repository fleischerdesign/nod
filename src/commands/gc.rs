//! `nod gc` command: collect garbage and delete old generations across the fleet (ADR-015).

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::collect_garbage::CollectGarbageUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;
use crate::domain::generation::GcOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    options: GcOptions,
    verbose: bool,
    json: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let hosts = evaluator
        .discover_hosts_degraded(flake_path, verbose)
        .await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let targets = resolve_targets(
        hosts,
        &local_hostname,
        target_args.target.as_deref(),
        target_args.tag.as_deref(),
        target_args.role.as_deref(),
        target_args.all,
        DefaultScope::All,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target_args.target.as_deref().unwrap_or("all"),
            target_args.tag.as_deref(),
            target_args.role.as_deref(),
        ));
    }

    let pb = if !json {
        let p = ProgressBar::new_spinner();
        p.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("  {spinner:.cyan} {msg}")
                .unwrap(),
        );
        let dry_prefix = if options.dry_run { "[dry-run] " } else { "" };
        p.set_message(format!(
            "{dry_prefix}Collecting garbage across {} target(s)...",
            targets.len()
        ));
        p.enable_steady_tick(Duration::from_millis(80));
        Some(p)
    } else {
        None
    };

    let use_case = CollectGarbageUseCase::new(Arc::new(ctx));
    let reports = use_case.execute(targets, options).await?;

    if let Some(p) = pb {
        p.finish_and_clear();
    }

    if json {
        println!("{}", serde_json::to_string(&reports).unwrap());
        return Ok(());
    }

    println!(
        "\n{:<22} {:<12} {}",
        "HOST".bold(),
        "STATUS".bold(),
        "SUMMARY".bold()
    );
    for report in &reports {
        let status_label = if report.success {
            "✓ ok".green()
        } else {
            "✗ failed".red()
        };
        println!(
            "{:<22} {:<12} {}",
            report.host_name.bold(),
            status_label,
            report.output_summary.dimmed()
        );
    }

    let success_count = reports.iter().filter(|r| r.success).count();
    println!(
        "\n  {}",
        format!(
            "{} of {} host(s) completed garbage collection.",
            success_count,
            reports.len()
        )
        .dimmed()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generation::{CopyOptions, CopyReport, GcReport, SystemGeneration};
    use crate::domain::host::{HostEntity, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::store::StorePort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeStorePort {}
        #[async_trait]
        impl StorePort for FakeStorePort {
            async fn list_generations(&self, host: &HostEntity, profile: &SshProfile) -> Result<Vec<SystemGeneration>, NodError>;
            async fn collect_garbage(&self, host: &HostEntity, profile: &SshProfile, options: &GcOptions) -> Result<GcReport, NodError>;
            async fn copy_closure(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, options: &CopyOptions) -> Result<CopyReport, NodError>;
        }
    }

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
        }
    }

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    #[tokio::test]
    async fn gc_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let mut store = MockFakeStorePort::new();
        store.expect_collect_garbage().returning(|host, _, _| {
            Ok(GcReport {
                host_name: host.name.clone(),
                success: true,
                output_summary: "1234 bytes freed".to_string(),
            })
        });

        let store_arc = Arc::new(store);
        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_stores(store_arc.clone(), store_arc);

        let res = execute(
            ctx,
            Path::new("."),
            &TargetArgs::default(),
            GcOptions::default(),
            false,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
