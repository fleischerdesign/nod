//! `nod copy` command: pre-stage store closures to target hosts without activation (ADR-015).

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::copy_closure::CopyClosureUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;
use crate::domain::generation::CopyOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    options: CopyOptions,
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
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target_args.target.as_deref().unwrap_or("local"),
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
        p.set_message(format!(
            "Building and copying closures to {} target(s)...",
            targets.len()
        ));
        p.enable_steady_tick(Duration::from_millis(80));
        Some(p)
    } else {
        None
    };
    let use_case = CopyClosureUseCase::new(Arc::new(ctx));
    let reports = use_case
        .execute(targets, flake_path, options, verbose)
        .await?;

    if let Some(p) = pb {
        p.finish_and_clear();
    }

    if json {
        println!("{}", serde_json::to_string(&reports).unwrap());
        return Ok(());
    }

    for report in &reports {
        let label = if report.success {
            "[copied]".green()
        } else {
            "[failed]".red()
        };
        println!(
            "  {} {} {}",
            report.host_name.bold(),
            label,
            report.closure_path.display().to_string().dimmed()
        );
    }

    let success_count = reports.iter().filter(|r| r.success).count();
    println!(
        "\n  {}",
        format!(
            "{} of {} host closure(s) copied successfully.",
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
    use crate::domain::generation::{CopyReport, GcOptions, GcReport, SystemGeneration};
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
    async fn copy_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("rollins", "100.126.5.72", false)]));
        eval.expect_build_toplevel()
            .returning(|_, host_name, _, _| {
                Ok(PathBuf::from(format!("/nix/store/test-{}", host_name)))
            });

        let mut store = MockFakeStorePort::new();
        store
            .expect_copy_closure()
            .returning(|host, _, closure, _| {
                Ok(CopyReport {
                    host_name: host.name.clone(),
                    closure_path: closure.to_path_buf(),
                    success: true,
                })
            });

        let store_arc = Arc::new(store);
        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_stores(store_arc.clone(), store_arc);

        let target_args = TargetArgs {
            target: Some("rollins".to_string()),
            ..Default::default()
        };
        let res = execute(
            ctx,
            Path::new("."),
            &target_args,
            CopyOptions::default(),
            false,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
