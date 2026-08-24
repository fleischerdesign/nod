//! `InspectMetadataUseCase`: inspect flake metadata over `FlakePort` (ADR-014).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::flake::FlakeMetadata;

/// Use case that queries high-level flake metadata and repository status.
pub struct InspectMetadataUseCase {
    ctx: Arc<AppContext>,
}

impl InspectMetadataUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(&self, flake_path: &Path) -> Result<FlakeMetadata, NodError> {
        let port = self.ctx.flake_port()?;
        port.load_metadata(flake_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::flake::{FlakeInputNode, FlakeUpdateReport};
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
    async fn inspect_metadata_delegates_to_port() {
        let mut flake = MockFakeFlakePort::new();
        flake.expect_load_metadata().returning(|_| {
            Ok(FlakeMetadata {
                path: "/etc/nixos".to_string(),
                url: None,
                revision: Some("e22212867d89909a438af05fbd9a9b3c9dbd3d0b".to_string()),
                rev_count: Some(1663),
                last_modified: Some(1787600890),
                lock_version: 7,
                total_inputs: 15,
                direct_inputs: 8,
            })
        });

        let ctx = Arc::new(
            AppContext::new(
                Arc::new(MockFakeEvaluator::new()),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_flake_port(Arc::new(flake)),
        );

        let use_case = InspectMetadataUseCase::new(ctx);
        let meta = use_case.execute(Path::new(".")).await.unwrap();
        assert_eq!(meta.path, "/etc/nixos");
        assert_eq!(meta.rev_count, Some(1663));
        assert_eq!(meta.total_inputs, 15);
    }
}
