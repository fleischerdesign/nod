//! `nod metadata` command: inspect flake repository and lockfile metadata (ADR-014).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::inspect_metadata::InspectMetadataUseCase;
use crate::domain::errors::NodError;

pub async fn execute(ctx: AppContext, flake_path: &Path, json: bool) -> Result<(), NodError> {
    let use_case = InspectMetadataUseCase::new(Arc::new(ctx));
    let meta = use_case.execute(flake_path).await?;

    if json {
        println!("{}", serde_json::to_string(&meta).unwrap());
        return Ok(());
    }

    println!("\n{}", "Flake Repository & Lockfile Metadata".bold());
    println!("{:<24} {}", "Path:".bold(), meta.path);

    if let Some(url) = meta.url {
        println!("{:<24} {}", "URL:".bold(), url);
    }

    if let Some(rev) = meta.revision {
        let rev_str = if rev.len() >= 7 { &rev[..7] } else { &rev };
        let count_str = meta
            .rev_count
            .map(|c| format!(" (rev #{c})"))
            .unwrap_or_default();
        println!(
            "{:<24} {}{}",
            "Revision:".bold(),
            rev_str.cyan(),
            count_str.dimmed()
        );
    }

    if let Some(last_mod) = meta.last_modified {
        println!(
            "{:<24} {}",
            "Last Modified:".bold(),
            format_epoch_utc(last_mod).dimmed()
        );
    }

    println!("{:<24} {}", "Lockfile Version:".bold(), meta.lock_version);
    println!(
        "{:<24} {} ({} direct, {} transitive)",
        "Input Nodes:".bold(),
        meta.total_inputs.to_string().bold(),
        meta.direct_inputs.to_string().green(),
        (meta.total_inputs.saturating_sub(meta.direct_inputs))
            .to_string()
            .dimmed()
    );

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
    use crate::domain::flake::{FlakeInputNode, FlakeMetadata, FlakeUpdateReport};
    use crate::domain::host::{HostEntity, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::flake::FlakePort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeFlakePort {}
        #[async_trait]
        impl FlakePort for FakeFlakePort {
            async fn load_metadata(&self, flake_path: &Path) -> Result<FlakeMetadata, NodError>;
            async fn load_inputs(&self, flake_path: &Path) -> Result<Vec<FlakeInputNode>, NodError>;
            async fn update_inputs(&self, flake_path: &Path, inputs: &[String]) -> Result<FlakeUpdateReport, NodError>;
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
    async fn metadata_command_runs_successfully() {
        let mut flake = MockFakeFlakePort::new();
        flake.expect_load_metadata().returning(|_| {
            Ok(FlakeMetadata {
                path: "/etc/nixos".to_string(),
                url: Some("git+file:///etc/nixos".to_string()),
                revision: Some("e22212867d89909a438af05fbd9a9b3c9dbd3d0b".to_string()),
                rev_count: Some(1663),
                last_modified: Some(1787600890),
                lock_version: 7,
                total_inputs: 15,
                direct_inputs: 8,
            })
        });

        let ctx = AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_flake_port(Arc::new(flake));

        let res = execute(ctx, Path::new("."), false).await;
        assert!(res.is_ok());
    }
}
