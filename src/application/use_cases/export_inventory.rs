//! `ExportInventoryUseCase`: export fleet inventory to Ansible, Prometheus or JSON (ADR-020).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::topology::{ExportFormat, FleetNode, FleetTopology};

/// Use case that exports the discovered fleet to standard configuration formats.
pub struct ExportInventoryUseCase {
    ctx: Arc<AppContext>,
}

impl ExportInventoryUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        flake_path: &Path,
        format: ExportFormat,
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
            ExportFormat::Ansible => Ok(topology.to_ansible_inventory()),
            ExportFormat::Prometheus => Ok(topology.to_prometheus_sd()),
            ExportFormat::Json => Ok(serde_json::to_string_pretty(&topology).unwrap()),
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
    async fn export_inventory_generates_ansible_yaml() {
        let mut eval_mock = MockFakeEvaluator::new();
        eval_mock
            .expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));

        let ctx = Arc::new(AppContext::new(
            Arc::new(eval_mock),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = ExportInventoryUseCase::new(ctx);
        let out = use_case
            .execute(Path::new("."), ExportFormat::Ansible, false)
            .await
            .unwrap();
        assert!(out.contains("yorke:"));
        assert!(out.contains("ansible_host: \"127.0.0.1\""));
    }
}
