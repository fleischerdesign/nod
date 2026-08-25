//! `nod generations` command: list installed profile generations across the fleet (ADR-015).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::list_generations::ListGenerationsUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
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

    let use_case = ListGenerationsUseCase::new(Arc::new(ctx));
    let host_generations = use_case.execute(&targets).await?;

    if json {
        println!("{}", serde_json::to_string(&host_generations).unwrap());
        return Ok(());
    }

    for host_gen in host_generations {
        println!(
            "\n{} ({})",
            host_gen.host_name.bold(),
            format!("{} generation(s)", host_gen.generations.len()).dimmed()
        );
        if host_gen.generations.is_empty() {
            println!("  {}", "No profile generations found.".dimmed());
            continue;
        }

        println!(
            "  {:<6} {:<10} {:<24} {}",
            "GEN".bold(),
            "ACTIVE".bold(),
            "CREATED (UTC)".bold(),
            "CLOSURE PATH".bold()
        );

        for gen in host_gen.generations {
            let active_label = if gen.is_current {
                "● current".green()
            } else {
                "  -".dimmed()
            };
            let date_str = gen
                .created_at
                .map(format_epoch_utc)
                .unwrap_or_else(|| "-".to_string());

            println!(
                "  {:<6} {:<10} {:<24} {}",
                gen.generation.to_string().bold(),
                active_label,
                date_str.dimmed(),
                gen.closure_path.display().to_string().cyan()
            );
        }
    }

    Ok(())
}

fn format_epoch_utc(secs: u64) -> String {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let mut d = secs / 86400;

    let mut year = 1970;
    loop {
        let leap = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            1
        } else {
            0
        };
        let days_in_year = 365 + leap;
        if d < days_in_year {
            let month_days = [31, 28 + leap, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            for (month, &md) in (1..).zip(month_days.iter()) {
                if d < md {
                    let day = d + 1;
                    return format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02} UTC");
                }
                d -= md;
            }
            break;
        }
        d -= days_in_year;
        year += 1;
    }

    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generation::{
        CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration,
    };
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
    async fn generations_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let mut store = MockFakeStorePort::new();
        store.expect_list_generations().returning(|host, _| {
            Ok(vec![SystemGeneration {
                generation: 120,
                is_current: true,
                created_at: Some(1787560497),
                closure_path: PathBuf::from(format!("/nix/store/test-{}", host.name)),
            }])
        });

        let store_arc = Arc::new(store);
        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_stores(store_arc.clone(), store_arc);

        let res = execute(ctx, Path::new("."), &TargetArgs::default(), false, false).await;
        assert!(res.is_ok());
    }
}
