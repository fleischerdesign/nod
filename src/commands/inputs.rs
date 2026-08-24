//! `nod inputs` command: inspect declared and locked flake inputs (ADR-014).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::list_inputs::ListInputsUseCase;
use crate::domain::errors::NodError;

pub async fn execute(ctx: AppContext, flake_path: &Path, json: bool) -> Result<(), NodError> {
    let use_case = ListInputsUseCase::new(Arc::new(ctx));
    let inputs = use_case.execute(flake_path).await?;

    if json {
        println!("{}", serde_json::to_string(&inputs).unwrap());
        return Ok(());
    }

    if inputs.is_empty() {
        println!("{}", "No flake inputs found.".dimmed());
        return Ok(());
    }

    println!(
        "\n{:<22} {:<12} {:<42} {:<10} {:<24}",
        "INPUT".bold(),
        "TYPE".bold(),
        "ORIGINAL SOURCE".bold(),
        "REV".bold(),
        "LAST MODIFIED (UTC)".bold()
    );

    for node in inputs {
        let type_label = if node.is_direct {
            "direct".green()
        } else {
            "transitive".dimmed()
        };
        let rev_str = node.short_rev().unwrap_or("-");
        let date_str = node
            .last_modified
            .map(format_epoch_utc)
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<22} {:<12} {:<42} {:<10} {}",
            node.name.bold(),
            type_label,
            node.original_url,
            rev_str.cyan(),
            date_str.dimmed()
        );
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
    async fn inputs_command_runs_successfully() {
        let mut flake = MockFakeFlakePort::new();
        flake.expect_load_inputs().returning(|_| {
            Ok(vec![FlakeInputNode {
                name: "nod".to_string(),
                original_url: "github:fleischerdesign/nod".to_string(),
                locked_rev: Some("0216c5dc8c05521a6ba798e541882222431d0990".to_string()),
                locked_ref: Some("develop".to_string()),
                last_modified: Some(1787600890),
                nar_hash: None,
                follows: vec![],
                is_direct: true,
            }])
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
