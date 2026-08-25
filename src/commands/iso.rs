//! `nod iso` command: build bootable installer ISO/image (ADR-021).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::generate_iso::GenerateIsoUseCase;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::provision::IsoOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_name: Option<&str>,
    options: IsoOptions,
    verbose: bool,
    json: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;

    let host = if let Some(name) = target_name {
        hosts.into_iter().find(|h| h.name == name).ok_or_else(|| {
            NodError::not_found(format!(
                "host '{}' not found in flake '{}'",
                name,
                flake_path.display()
            ))
        })?
    } else {
        hosts
            .into_iter()
            .next()
            .unwrap_or_else(|| HostEntity::new("installer", "127.0.0.1", true))
    };

    if !json {
        println!(
            "Building bootable {} for host '{}'...",
            options.target_format.cyan(),
            host.name.bold()
        );
    }

    let use_case = GenerateIsoUseCase::new(Arc::new(ctx));
    let report = use_case.execute(&host, flake_path, &options).await?;

    if json {
        println!("{}", serde_json::to_string(&report).unwrap());
        return Ok(());
    }

    if report.ok {
        println!(
            "\n{} Bootable image built successfully:\n  {}",
            "✓".green().bold(),
            report.out_path.display().to_string().bold().cyan()
        );
    } else {
        println!(
            "\n{} Image build failed: {}",
            "✗".red().bold(),
            report.error.as_deref().unwrap_or("unknown error").red()
        );
        return Err(NodError::build_failure(
            report.host_name,
            report.error.unwrap_or_default(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{HostEntity, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::provisioner::ProvisionerPort;
    use crate::domain::provision::{BootstrapOptions, BootstrapReport, IsoReport};
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
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
        FakeProvisionerPort {}
        #[async_trait]
        impl ProvisionerPort for FakeProvisionerPort {
            async fn bootstrap(&self, host: &HostEntity, flake_path: &Path, options: &BootstrapOptions) -> Result<BootstrapReport, NodError>;
            async fn build_iso(&self, host: &HostEntity, flake_path: &Path, options: &IsoOptions) -> Result<IsoReport, NodError>;
        }
    }

    #[tokio::test]
    async fn iso_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("installer", "127.0.0.1", true)]));

        let mut prov = MockFakeProvisionerPort::new();
        prov.expect_build_iso().returning(|host, _, _| {
            Ok(IsoReport {
                host_name: host.name.clone(),
                out_path: PathBuf::from("/nix/store/iso-output.iso"),
                ok: true,
                error: None,
            })
        });

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_provisioner_port(Arc::new(prov));

        let res = execute(
            ctx,
            Path::new("."),
            Some("installer"),
            IsoOptions::default(),
            false,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
