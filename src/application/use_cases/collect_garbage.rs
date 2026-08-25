//! `CollectGarbageUseCase`: run garbage collection across targets bounded by concurrency (ADR-015).

use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::generation::{GcOptions, GcReport};
use crate::domain::host::HostEntity;

/// Use case that orchestrates garbage collection across fleet hosts.
pub struct CollectGarbageUseCase {
    ctx: Arc<AppContext>,
}

impl CollectGarbageUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: Vec<HostEntity>,
        options: GcOptions,
    ) -> Result<Vec<GcReport>, NodError> {
        let concurrency = options.concurrency.max(1);
        let sem = Arc::new(Semaphore::new(concurrency));
        let mut set = JoinSet::<Result<GcReport, NodError>>::new();

        for host in targets {
            let sem_c = sem.clone();
            let ctx = self.ctx.clone();
            let options = options.clone();

            set.spawn(async move {
                let _permit = sem_c.acquire().await.ok();
                let store = ctx.store_for(&host)?;
                let profile = ctx.resolved_profile(&host).await?;
                store.collect_garbage(&host, &profile, &options).await
            });
        }

        let mut reports = Vec::new();
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(report)) => reports.push(report),
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(NodError::internal(format!("task join error: {e}"))),
            }
        }

        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generation::{CopyOptions, CopyReport, SystemGeneration};
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
    async fn collect_garbage_runs_across_targets() {
        let mut store = MockFakeStorePort::new();
        store.expect_collect_garbage().returning(|host, _, _| {
            Ok(GcReport {
                host_name: host.name.clone(),
                success: true,
                output_summary: "1234 bytes freed".to_string(),
            })
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

        let use_case = CollectGarbageUseCase::new(ctx);
        let targets = vec![
            HostEntity::new("yorke", "127.0.0.1", true),
            HostEntity::new("rollins", "100.126.5.72", false),
        ];

        let results = use_case
            .execute(targets, GcOptions::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }
}
