//! `nod secret` command: pre-flight secret verification and fleet rekeying (ADR-018).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::check_secrets::CheckSecretsUseCase;
use crate::application::use_cases::rekey_secrets::RekeySecretsUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;
use crate::domain::secret::RekeyOptions;

pub async fn execute_check(
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
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target_args.target.as_deref().unwrap_or("local"),
            target_args.tag.as_deref(),
            target_args.role.as_deref(),
        ));
    }

    let use_case = CheckSecretsUseCase::new(Arc::new(ctx));
    let reports = use_case.execute(&targets, flake_path).await?;

    if json {
        println!("{}", serde_json::to_string(&reports).unwrap());
        return Ok(());
    }

    let mut failed_hosts = 0;
    for r in &reports {
        let status = if r.valid {
            "✓ valid".green()
        } else {
            failed_hosts += 1;
            "✗ invalid".red()
        };

        println!(
            "\n{} ({}, {} secret(s)) — {}",
            r.host_name.bold(),
            r.provider.to_string().cyan(),
            r.secrets_count,
            status
        );

        for detail in &r.details {
            let secret_status = if detail.decryptable {
                "✓ decryptable".green()
            } else {
                "✗ failed".red()
            };

            println!(
                "  • {:<30} {:<15} {}",
                detail.name,
                secret_status,
                detail.path.display().to_string().dimmed()
            );

            if let Some(err) = &detail.error {
                println!("      {}", err.red().dimmed());
            }
        }
    }

    if failed_hosts > 0 {
        return Err(NodError::secret(format!(
            "{} host(s) contain invalid or undecryptable secrets",
            failed_hosts
        )));
    }

    Ok(())
}

pub async fn execute_rekey(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    options: RekeyOptions,
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

    let use_case = RekeySecretsUseCase::new(Arc::new(ctx));
    let reports = use_case.execute(&targets, flake_path, &options).await?;

    if json {
        println!("{}", serde_json::to_string(&reports).unwrap());
        return Ok(());
    }

    let mut failed_hosts = 0;
    for r in &reports {
        let status = if r.ok {
            "✓ rekeyed".green()
        } else {
            failed_hosts += 1;
            "✗ failed".red()
        };

        println!(
            "\n{} ({}, {} file(s)) — {}",
            r.host_name.bold(),
            r.provider.to_string().cyan(),
            r.files_rekeyed.len(),
            status
        );

        for path in &r.files_rekeyed {
            println!("  • {}", path.display().to_string().dimmed());
        }

        if let Some(err) = &r.error {
            println!("      {}", err.red().dimmed());
        }
    }

    if failed_hosts > 0 {
        return Err(NodError::secret(format!(
            "{} host(s) failed during secret rekeying",
            failed_hosts
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
    use crate::domain::ports::secret::SecretPort;
    use crate::domain::secret::{RekeyReport, SecretCheckReport, SecretProvider};
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
        FakeSecretPort {}
        #[async_trait]
        impl SecretPort for FakeSecretPort {
            async fn check_secrets(&self, host: &HostEntity, flake_path: &Path) -> Result<SecretCheckReport, NodError>;
            async fn rekey_secrets(&self, host: &HostEntity, flake_path: &Path, options: &RekeyOptions) -> Result<RekeyReport, NodError>;
        }
    }

    #[tokio::test]
    async fn check_secrets_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let mut secret = MockFakeSecretPort::new();
        secret.expect_check_secrets().returning(|host, _| {
            Ok(SecretCheckReport {
                host_name: host.name.clone(),
                provider: SecretProvider::None,
                secrets_count: 0,
                valid: true,
                details: Vec::new(),
            })
        });

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        )
        .with_secret_port(Arc::new(secret));

        let res = execute_check(ctx, Path::new("."), &TargetArgs::default(), false, false).await;
        assert!(res.is_ok());
    }
}
