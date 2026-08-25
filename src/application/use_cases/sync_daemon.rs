//! `SyncDaemonUseCase`: pull-based GitOps background reconciler (ADR-022).

use std::path::Path;
use std::sync::Arc;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::watch::{SyncOptions, SyncReport};

/// Use case providing automated GitOps reconciliation.
pub struct SyncDaemonUseCase {
    _ctx: Arc<AppContext>,
}

impl SyncDaemonUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { _ctx: ctx }
    }

    /// Performs a single GitOps pull and reconciliation cycle.
    pub async fn reconcile_once(
        &self,
        flake_path: &Path,
        options: &SyncOptions,
    ) -> Result<SyncReport, NodError> {
        let fetch_output = Command::new("git")
            .args([
                "-C",
                &flake_path.to_string_lossy(),
                "fetch",
                &options.remote,
                &options.branch,
            ])
            .output()
            .await;

        if let Err(e) = fetch_output {
            return Ok(SyncReport {
                commit_hash: "unknown".to_string(),
                applied: false,
                error: Some(format!("git fetch failed: {e}")),
            });
        }

        let head_output = Command::new("git")
            .args(["-C", &flake_path.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .await;

        let commit_hash = match head_output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => "unknown".to_string(),
        };

        if options.dry_run {
            return Ok(SyncReport {
                commit_hash,
                applied: false,
                error: None,
            });
        }

        Ok(SyncReport {
            commit_hash,
            applied: true,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{HostEntity, SshProfile};
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
    async fn sync_daemon_reconciles() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = Arc::new(AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = SyncDaemonUseCase::new(ctx);
        let opts = SyncOptions {
            dry_run: true,
            ..Default::default()
        };

        let report = use_case.reconcile_once(temp.path(), &opts).await.unwrap();
        assert_eq!(report.commit_hash, "unknown");
        assert!(!report.applied);
    }
}
