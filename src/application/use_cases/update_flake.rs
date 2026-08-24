//! `UpdateFlakeUseCase`: update flake inputs over `FlakePort` and compute revision deltas (ADR-014).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::flake::FlakeUpdateReport;

/// Use case that updates flake inputs and reports calculated deltas.
pub struct UpdateFlakeUseCase {
    ctx: Arc<AppContext>,
}

impl UpdateFlakeUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        flake_path: &Path,
        inputs: &[String],
    ) -> Result<FlakeUpdateReport, NodError> {
        let port = self.ctx.flake_port()?;
        port.update_inputs(flake_path, inputs).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::flake::{FlakeInputNode, FlakeMetadata, InputDelta};
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
    async fn update_flake_delegates_to_port() {
        let mut flake = MockFakeFlakePort::new();
        flake
            .expect_update_inputs()
            .with(
                mockall::predicate::always(),
                mockall::predicate::eq(vec!["nod".to_string()]),
            )
            .returning(|_, _| {
                Ok(FlakeUpdateReport::from_deltas(vec![InputDelta {
                    name: "nod".to_string(),
                    old_rev: Some("510c94e".to_string()),
                    new_rev: Some("c1cc7a0".to_string()),
                    old_last_modified: Some(100),
                    new_last_modified: Some(200),
                }]))
            });

        let ctx = Arc::new(
            AppContext::new(
                Arc::new(MockFakeEvaluator::new()),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_flake_port(Arc::new(flake)),
        );

        let use_case = UpdateFlakeUseCase::new(ctx);
        let report = use_case
            .execute(Path::new("."), &["nod".to_string()])
            .await
            .unwrap();
        assert_eq!(report.updated_count, 1);
        assert_eq!(report.deltas[0].name, "nod");
    }
}
