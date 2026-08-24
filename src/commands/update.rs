//! `nod update` command: update flake inputs and display revision deltas (ADR-014).

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::application::context::AppContext;
use crate::application::use_cases::update_flake::UpdateFlakeUseCase;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    inputs: Vec<String>,
    json: bool,
) -> Result<(), NodError> {
    let target_label = if inputs.is_empty() {
        "all inputs".to_string()
    } else {
        inputs.join(", ")
    };

    let pb = if !json {
        let p = ProgressBar::new_spinner();
        p.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("  {spinner:.cyan} {msg}")
                .unwrap(),
        );
        p.set_message(format!("Updating flake inputs ({target_label})..."));
        p.enable_steady_tick(Duration::from_millis(80));
        Some(p)
    } else {
        None
    };

    let use_case = UpdateFlakeUseCase::new(Arc::new(ctx));
    let report = use_case.execute(flake_path, &inputs).await?;

    if let Some(p) = pb {
        p.finish_and_clear();
    }

    if json {
        println!("{}", serde_json::to_string(&report).unwrap());
        return Ok(());
    }

    if report.unchanged {
        println!(
            "  {}",
            "✓ All inputs are already up to date.".bold().green()
        );
        return Ok(());
    }

    println!(
        "\n{:<22} {:<12} {:<12}",
        "UPDATED INPUT".bold(),
        "OLD REV".bold(),
        "NEW REV".bold()
    );

    for delta in &report.deltas {
        let old_rev = delta.short_old_rev().unwrap_or("-");
        let new_rev = delta.short_new_rev().unwrap_or("-");
        println!(
            "{:<22} {:<12} {}",
            delta.name.bold(),
            old_rev.yellow(),
            new_rev.green()
        );
    }

    println!(
        "\n  {}",
        format!(
            "✓ Successfully updated {} flake input(s).",
            report.updated_count
        )
        .green()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::flake::{FlakeInputNode, FlakeMetadata, FlakeUpdateReport, InputDelta};
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
    async fn update_command_runs_successfully() {
        let mut flake = MockFakeFlakePort::new();
        flake.expect_update_inputs().returning(|_, _| {
            Ok(FlakeUpdateReport::from_deltas(vec![InputDelta {
                name: "nod".to_string(),
                old_rev: Some("510c94e".to_string()),
                new_rev: Some("c1cc7a0".to_string()),
                old_last_modified: Some(100),
                new_last_modified: Some(200),
            }]))
        });

        let ctx = AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_flake_port(Arc::new(flake));

        let res = execute(ctx, Path::new("."), vec!["nod".to_string()], false).await;
        assert!(res.is_ok());
    }
}
