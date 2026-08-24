//! `ListInputsUseCase`: read resolved flake input nodes over `FlakePort` (ADR-014).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::flake::FlakeInputNode;

/// Use case that queries input nodes declared in a flake.
pub struct ListInputsUseCase {
    ctx: Arc<AppContext>,
}

impl ListInputsUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(&self, flake_path: &Path) -> Result<Vec<FlakeInputNode>, NodError> {
        let port = self.ctx.flake_port()?;
        port.load_inputs(flake_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::flake::FlakeMetadata;
    use crate::domain::flake::FlakeUpdateReport;
    use crate::domain::host::HostEntity;
    use crate::domain::host::SshProfile;
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
    async fn list_inputs_delegates_to_port() {
        let mut flake = MockFakeFlakePort::new();
        flake.expect_load_inputs().returning(|_| {
            Ok(vec![FlakeInputNode {
                name: "nixpkgs".to_string(),
                original_url: "github:NixOS/nixpkgs".to_string(),
                locked_rev: Some("a831408e6378bc02ebf8cc09b52c96ca86f6bab4".to_string()),
                locked_ref: Some("nixpkgs-unstable".to_string()),
                last_modified: Some(1787364730),
                nar_hash: None,
                follows: vec![],
                is_direct: true,
            }])
        });

        let ctx = Arc::new(
            AppContext::new(
                Arc::new(MockFakeEvaluator::new()),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_flake_port(Arc::new(flake)),
        );

        let use_case = ListInputsUseCase::new(ctx);
        let inputs = use_case.execute(Path::new(".")).await.unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "nixpkgs");
    }
}
