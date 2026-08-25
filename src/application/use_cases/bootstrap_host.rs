//! `BootstrapHostUseCase`: bootstrap bare-metal host using nixos-anywhere (ADR-021).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::provision::{BootstrapOptions, BootstrapReport};

/// Use case that bootstraps a remote machine via `nixos-anywhere`.
pub struct BootstrapHostUseCase {
    ctx: Arc<AppContext>,
}

impl BootstrapHostUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        target: &HostEntity,
        flake_path: &Path,
        options: &BootstrapOptions,
    ) -> Result<BootstrapReport, NodError> {
        let provisioner = self.ctx.provisioner_port()?;
        provisioner.bootstrap(target, flake_path, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::provisioner::ProvisionerPort;
    use crate::domain::provision::{IsoOptions, IsoReport};
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
    async fn bootstrap_executes_on_target() {
        let mut prov_mock = MockFakeProvisionerPort::new();
        prov_mock.expect_bootstrap().returning(|host, _, opts| {
            Ok(BootstrapReport {
                host_name: host.name.clone(),
                target_ip: opts.target_ip.clone(),
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
            .with_provisioner_port(Arc::new(prov_mock)),
        );

        let use_case = BootstrapHostUseCase::new(ctx);
        let host = HostEntity::new("selway", "10.0.0.50", false);
        let opts = BootstrapOptions {
            target_ip: "10.0.0.50".to_string(),
            ..Default::default()
        };

        let report = use_case
            .execute(&host, Path::new("."), &opts)
            .await
            .unwrap();
        assert_eq!(report.host_name, "selway");
        assert!(report.ok);
    }
}
