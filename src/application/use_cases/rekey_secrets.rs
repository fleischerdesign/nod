//! `RekeySecretsUseCase`: re-encrypt secrets with updated recipient keys per host (ADR-018).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::secret::{RekeyOptions, RekeyReport};

/// Use case that re-encrypts secret files across target hosts.
pub struct RekeySecretsUseCase {
    ctx: Arc<AppContext>,
}

impl RekeySecretsUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: &[HostEntity],
        flake_path: &Path,
        options: &RekeyOptions,
    ) -> Result<Vec<RekeyReport>, NodError> {
        let port = self.ctx.secret_port()?;
        let mut reports = Vec::with_capacity(targets.len());

        for host in targets {
            let report = port.rekey_secrets(host, flake_path, options).await?;
            reports.push(report);
        }

        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::secret::SecretPort;
    use crate::domain::secret::{SecretCheckReport, SecretProvider};
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
    async fn rekey_secrets_returns_reports_for_targets() {
        let mut secret_mock = MockFakeSecretPort::new();
        secret_mock.expect_rekey_secrets().returning(|host, _, _| {
            Ok(RekeyReport {
                host_name: host.name.clone(),
                provider: SecretProvider::None,
                files_rekeyed: Vec::new(),
                ok: true,
                error: None,
            })
        });

        let ctx = Arc::new(
            AppContext::new(
                Arc::new(MockFakeEvaluator::new()),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_secret_port(Arc::new(secret_mock)),
        );

        let use_case = RekeySecretsUseCase::new(ctx);
        let targets = vec![HostEntity::new("yorke", "127.0.0.1", true)];
        let options = RekeyOptions::default();

        let reports = use_case
            .execute(&targets, Path::new("."), &options)
            .await
            .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].host_name, "yorke");
        assert!(reports[0].ok);
    }
}
