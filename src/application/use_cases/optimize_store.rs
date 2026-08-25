//! `OptimizeStoreUseCase`: execute hardlink deduplication on Nix stores across hosts (ADR-019).

use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::cache::StoreOptimizeReport;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Use case that executes `nix-store --optimise` across target hosts.
pub struct OptimizeStoreUseCase {
    ctx: Arc<AppContext>,
}

impl OptimizeStoreUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: &[HostEntity],
    ) -> Result<Vec<StoreOptimizeReport>, NodError> {
        let mut reports = Vec::with_capacity(targets.len());

        for host in targets {
            let profile = self.ctx.resolved_profile(host).await?;
            let store = self.ctx.store_for(host)?;
            let report = store.optimize_store(host, &profile).await?;
            reports.push(report);
        }

        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cache::CachePushReport;
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
        FakeStorePort {}
        #[async_trait]
        impl StorePort for FakeStorePort {
            async fn list_generations(&self, host: &HostEntity, profile: &SshProfile) -> Result<Vec<SystemGeneration>, NodError>;
            async fn collect_garbage(&self, host: &HostEntity, profile: &SshProfile, options: &GcOptions) -> Result<GcReport, NodError>;
            async fn copy_closure(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, options: &CopyOptions) -> Result<CopyReport, NodError>;
            async fn optimize_store(&self, host: &HostEntity, profile: &SshProfile) -> Result<StoreOptimizeReport, NodError>;
            async fn push_cache(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, cache_uri: &str) -> Result<CachePushReport, NodError>;
        }
    }

    #[tokio::test]
    async fn optimize_store_executes_across_targets() {
        let mut store_mock = MockFakeStorePort::new();
        store_mock.expect_optimize_store().returning(|host, _| {
            Ok(StoreOptimizeReport {
                host_name: host.name.clone(),
                ok: true,
                freed_bytes: None,
                error: None,
            })
        });

        let store_arc = Arc::new(store_mock);
        let ctx = Arc::new(
            AppContext::new(
                Arc::new(MockFakeEvaluator::new()),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_stores(store_arc.clone(), store_arc),
        );

        let use_case = OptimizeStoreUseCase::new(ctx);
        let targets = vec![HostEntity::new("yorke", "127.0.0.1", true)];

        let reports = use_case.execute(&targets).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].host_name, "yorke");
        assert!(reports[0].ok);
    }
}
