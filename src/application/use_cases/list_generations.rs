//! `ListGenerationsUseCase`: collect profile generations across resolved fleet targets (ADR-015).

use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::generation::HostGenerations;
use crate::domain::host::HostEntity;

/// Use case that queries system profile generations per target host.
pub struct ListGenerationsUseCase {
    ctx: Arc<AppContext>,
}

impl ListGenerationsUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(&self, targets: &[HostEntity]) -> Result<Vec<HostGenerations>, NodError> {
        let mut results = Vec::with_capacity(targets.len());

        for host in targets {
            let store = self.ctx.store_for(host)?;
            let profile = self.ctx.resolved_profile(host).await?;
            let generations = store.list_generations(host, &profile).await?;

            results.push(HostGenerations {
                host_name: host.name.clone(),
                generations,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generation::{
        CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration,
    };
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::store::StorePort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::{Path, PathBuf};

    mock! {
        FakeStorePort {}
        #[async_trait]
        impl StorePort for FakeStorePort {
            async fn list_generations(&self, host: &HostEntity, profile: &SshProfile) -> Result<Vec<SystemGeneration>, NodError>;
            async fn collect_garbage(&self, host: &HostEntity, profile: &SshProfile, options: &GcOptions) -> Result<GcReport, NodError>;
            async fn copy_closure(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, options: &CopyOptions) -> Result<CopyReport, NodError>;
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
    async fn list_generations_aggregates_across_targets() {
        let mut store = MockFakeStorePort::new();
        store.expect_list_generations().returning(|host, _| {
            Ok(vec![SystemGeneration {
                generation: 120,
                is_current: true,
                created_at: Some(1787560497),
                closure_path: PathBuf::from(format!("/nix/store/test-{}", host.name)),
            }])
        });

        let store_arc = Arc::new(store);
        let ctx = Arc::new(
            AppContext::new(
                Arc::new(MockFakeEvaluator::new()),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_stores(store_arc.clone(), store_arc),
        );

        let use_case = ListGenerationsUseCase::new(ctx);
        let targets = vec![
            HostEntity::new("yorke", "127.0.0.1", true),
            HostEntity::new("rollins", "100.126.5.72", false),
        ];

        let results = use_case.execute(&targets).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].generations[0].generation, 120);
        assert_eq!(results[1].generations[0].generation, 120);
    }
}
