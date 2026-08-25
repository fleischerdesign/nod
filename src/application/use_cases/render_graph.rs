//! `RenderGraphUseCase`: render fleet topology diagrams (ADR-020).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::topology::{FleetNode, FleetTopology, GraphFormat};

/// Use case that generates a graph representation of the fleet topology.
pub struct RenderGraphUseCase {
    ctx: Arc<AppContext>,
}

impl RenderGraphUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        flake_path: &Path,
        format: GraphFormat,
        verbose: bool,
    ) -> Result<String, NodError> {
        let evaluator = self.ctx.evaluator();
        let hosts = evaluator
            .discover_hosts_degraded(flake_path, verbose)
            .await?;

        let nodes: Vec<FleetNode> = hosts
            .into_iter()
            .map(|h| FleetNode {
                name: h.name,
                target_host: h.target_host,
                tags: h.tags,
                role: Some(h.role.to_str()),
                system: None,
                is_local: h.is_local,
            })
            .collect();

        let topology = FleetTopology {
            nodes,
            flake_uri: flake_path.display().to_string(),
        };

        match format {
            GraphFormat::Mermaid => Ok(topology.to_mermaid()),
            GraphFormat::Dot => Ok(topology.to_dot()),
            GraphFormat::Json => Ok(serde_json::to_string_pretty(&topology).unwrap()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostEntity;
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
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

    #[tokio::test]
    async fn render_graph_generates_mermaid_diagram() {
        let mut eval_mock = MockFakeEvaluator::new();
        eval_mock
            .expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let ctx = Arc::new(AppContext::new(
            Arc::new(eval_mock),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = RenderGraphUseCase::new(ctx);
        let out = use_case
            .execute(Path::new("."), GraphFormat::Mermaid, false)
            .await
            .unwrap();
        assert!(out.contains("graph TD"));
        assert!(out.contains("yorke"));
    }
}
