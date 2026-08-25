//! `PushCacheUseCase`: push built system closures to a remote binary cache (ADR-019).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::cache::{CachePushOptions, CachePushReport};
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Use case that pushes compiled system closures to a binary cache.
pub struct PushCacheUseCase {
    ctx: Arc<AppContext>,
}

impl PushCacheUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: &[HostEntity],
        flake_path: &Path,
        options: &CachePushOptions,
        verbose: bool,
    ) -> Result<Vec<CachePushReport>, NodError> {
        let evaluator = self.ctx.evaluator();
        let mut reports = Vec::with_capacity(targets.len());

        for host in targets {
            let profile = self.ctx.resolved_profile(host).await?;
            let store = self.ctx.store_for(host)?;

            let cache_uri = if let Some(ref uri) = options.cache_uri {
                uri.clone()
            } else if let Some(ref substituters) = host.nod_config.build.substituters {
                if let Some(first) = substituters.first() {
                    first.clone()
                } else {
                    return Err(NodError::config(format!(
                        "no binary cache URI specified for host '{}' (provide --cache <URI>)",
                        host.name
                    )));
                }
            } else {
                return Err(NodError::config(format!(
                    "no binary cache URI specified for host '{}' (provide --cache <URI>)",
                    host.name
                )));
            };

            let closure = evaluator
                .build_toplevel(flake_path, &host.name, None, verbose)
                .await?;

            if options.dry_run {
                reports.push(CachePushReport {
                    host_name: host.name.clone(),
                    closure_path: closure,
                    cache_uri,
                    ok: true,
                    error: None,
                });
                continue;
            }

            let report = store
                .push_cache(host, &profile, &closure, &cache_uri)
                .await?;
            reports.push(report);
        }

        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cache::StoreOptimizeReport;
    use crate::domain::generation::{
        CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration,
    };
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::store::StorePort;
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
    async fn push_cache_builds_and_pushes_closure() {
        let mut eval_mock = MockFakeEvaluator::new();
        eval_mock
            .expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/test-closure")));

        let mut store_mock = MockFakeStorePort::new();
        store_mock
            .expect_push_cache()
            .returning(|host, _, closure, uri| {
                Ok(CachePushReport {
                    host_name: host.name.clone(),
                    closure_path: closure.to_path_buf(),
                    cache_uri: uri.to_string(),
                    ok: true,
                    error: None,
                })
            });

        let store_arc = Arc::new(store_mock);
        let ctx = Arc::new(
            AppContext::new(
                Arc::new(eval_mock),
                Arc::new(MockFakeDeployer::new()),
                Arc::new(MockFakeDeployer::new()),
            )
            .with_stores(store_arc.clone(), store_arc),
        );

        let use_case = PushCacheUseCase::new(ctx);
        let targets = vec![HostEntity::new("yorke", "127.0.0.1", true)];
        let options = CachePushOptions {
            cache_uri: Some("s3://test-cache".to_string()),
            dry_run: false,
            concurrency: 1,
        };

        let reports = use_case
            .execute(&targets, Path::new("."), &options, false)
            .await
            .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].host_name, "yorke");
        assert!(reports[0].ok);
    }
}
