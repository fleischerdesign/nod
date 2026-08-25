//! `CopyClosureUseCase`: build and pre-stage system closures to target hosts without activation (ADR-015).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::generation::{CopyOptions, CopyReport};
use crate::domain::host::HostEntity;

/// Use case that builds top-level closures and copies them to target machines.
pub struct CopyClosureUseCase {
    ctx: Arc<AppContext>,
}

impl CopyClosureUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: Vec<HostEntity>,
        flake_path: &Path,
        options: CopyOptions,
        verbose: bool,
    ) -> Result<Vec<CopyReport>, NodError> {
        let evaluator = self.ctx.evaluator();
        let mut reports = Vec::with_capacity(targets.len());

        for host in targets {
            let closure = evaluator
                .build_toplevel(flake_path, &host.name, None, verbose)
                .await?;

            let store = self.ctx.store_for(&host)?;
            let profile = self.ctx.resolved_profile(&host).await?;
            let report = store
                .copy_closure(&host, &profile, &closure, &options)
                .await?;
            reports.push(report);
        }

        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generation::{GcOptions, GcReport, SystemGeneration};
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::store::StorePort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

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
    async fn copy_closure_builds_and_copies() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .returning(|_, host_name, _, _| {
                Ok(PathBuf::from(format!("/nix/store/test-{}", host_name)))
            });

        let mut store = MockFakeStorePort::new();
        store
            .expect_copy_closure()
            .returning(|host, _, closure, _| {
                Ok(CopyReport {
                    host_name: host.name.clone(),
                    closure_path: closure.to_path_buf(),
                    success: true,
                })
            });

        let store_arc = Arc::new(store);
        let ctx = Arc::new(
            AppContext::new(
                Arc::new(eval),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_stores(store_arc.clone(), store_arc),
        );

        let use_case = CopyClosureUseCase::new(ctx);
        let targets = vec![HostEntity::new("rollins", "100.126.5.72", false)];

        let reports = use_case
            .execute(targets, Path::new("."), CopyOptions::default(), false)
            .await
            .unwrap();

        assert_eq!(reports.len(), 1);
        assert!(reports[0].success);
    }
}
