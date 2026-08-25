//! `nod bootstrap` command: bare-metal installation via nixos-anywhere (ADR-021).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::bootstrap_host::BootstrapHostUseCase;
use crate::domain::errors::NodError;
use crate::domain::provision::BootstrapOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_name: &str,
    options: BootstrapOptions,
    verbose: bool,
    json: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let hosts = evaluator.discover_hosts(flake_path, verbose).await?;

    let host = hosts
        .into_iter()
        .find(|h| h.name == target_name)
        .ok_or_else(|| {
            NodError::not_found(format!(
                "host '{}' not found in flake '{}'",
                target_name,
                flake_path.display()
            ))
        })?;

    if !json {
        println!(
            "Bootstrapping host '{}' onto {} (disko: {}, user: {})...",
            host.name.bold(),
            options.target_ip.cyan(),
            options.disko,
            options.ssh_user
        );
    }

    let use_case = BootstrapHostUseCase::new(Arc::new(ctx));
    let report = use_case.execute(&host, flake_path, &options).await?;

    if json {
        println!("{}", serde_json::to_string(&report).unwrap());
        return Ok(());
    }

    if report.ok {
        println!(
            "\n{} Successfully bootstrapped '{}' onto {}",
            "✓".green().bold(),
            report.host_name.bold(),
            report.target_ip.cyan()
        );
    } else {
        println!(
            "\n{} Bootstrapping failed for '{}': {}",
            "✗".red().bold(),
            report.host_name.bold(),
            report.error.as_deref().unwrap_or("unknown error").red()
        );
        return Err(NodError::deployment(format!(
            "failed to bootstrap host '{}'",
            report.host_name
        )));
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
    use crate::domain::provision::{BootstrapReport, IsoOptions, IsoReport};
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
    async fn bootstrap_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("selway", "10.0.0.50", false)]));

        let mut prov = MockFakeProvisionerPort::new();
        prov.expect_bootstrap().returning(|host, _, opts| {
            Ok(BootstrapReport {
                host_name: host.name.clone(),
                target_ip: opts.target_ip.clone(),
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

        let opts = BootstrapOptions {
            target_ip: "10.0.0.50".to_string(),
            ..Default::default()
        };

        let res = execute(ctx, Path::new("."), "selway", opts, false, false).await;
        assert!(res.is_ok());
    }
}
