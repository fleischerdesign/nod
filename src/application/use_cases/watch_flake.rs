//! `WatchFlakeUseCase`: live auto-preview rebuild and plan diff on file changes (ADR-022).

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::application::context::AppContext;
use crate::application::use_cases::generate_plan::GeneratePlanUseCase;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentOptions, DeploymentPlan};

/// Use case providing file change detection and live plan generation.
pub struct WatchFlakeUseCase {
    ctx: Arc<AppContext>,
}

impl WatchFlakeUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Computes the maximum modification time across all `.nix` and `.toml` files.
    pub fn compute_max_mtime(flake_path: &Path) -> Option<SystemTime> {
        let mut max_mtime = None;
        if let Ok(entries) = std::fs::read_dir(flake_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|s| s.to_str());
                    if matches!(ext, Some("nix") | Some("toml") | Some("lock")) {
                        if let Ok(meta) = path.metadata() {
                            if let Ok(mtime) = meta.modified() {
                                max_mtime = Some(match max_mtime {
                                    Some(curr) => std::cmp::max(curr, mtime),
                                    None => mtime,
                                });
                            }
                        }
                    }
                }
            }
        }
        max_mtime
    }

    /// Executes a single plan evaluation pass for `targets`.
    pub async fn evaluate_pass(
        &self,
        targets: &[HostEntity],
        flake_path: &Path,
        verbose: bool,
    ) -> Result<DeploymentPlan, NodError> {
        let plan_use_case = GeneratePlanUseCase::new(self.ctx.clone());
        let mut options = DeploymentOptions::default_policy();
        options.verbose = verbose;
        plan_use_case
            .plan(targets.to_vec(), options, flake_path)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn watch_flake_computes_mtime_and_evaluates() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("flake.nix"), "# test").unwrap();

        let mtime = WatchFlakeUseCase::compute_max_mtime(temp.path());
        assert!(mtime.is_some());

        let mut eval_mock = MockFakeEvaluator::new();
        eval_mock
            .expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/test-closure")));

        let mut deploy_mock = MockFakeDeployer::new();
        deploy_mock
            .expect_current_closure()
            .returning(|_, _| Ok(Some(PathBuf::from("/nix/store/old-closure"))));

        let ctx = Arc::new(AppContext::new(
            Arc::new(eval_mock),
            Arc::new(deploy_mock),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = WatchFlakeUseCase::new(ctx);
        let targets = vec![HostEntity::new("yorke", "127.0.0.1", true)];
        let plan = use_case
            .evaluate_pass(&targets, temp.path(), false)
            .await
            .unwrap();
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].host_name, "yorke");
    }
}
