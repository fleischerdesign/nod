//! `nod daemon` command: systemd background reconciler service (ADR-022).

use std::path::Path;

use crate::application::context::AppContext;
use crate::commands::sync::execute as execute_sync;
use crate::domain::errors::NodError;
use crate::domain::watch::SyncOptions;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    interval: u64,
    verbose: bool,
) -> Result<(), NodError> {
    let options = SyncOptions {
        interval_secs: interval,
        once: false,
        ..Default::default()
    };

    execute_sync(ctx, flake_path, options, verbose).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;
    use std::sync::Arc;

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<crate::domain::host::HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &crate::domain::host::HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &crate::domain::host::HostEntity, profile: &crate::domain::host::SshProfile) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &crate::domain::host::HostEntity, profile: &crate::domain::host::SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &crate::domain::host::HostEntity, profile: &crate::domain::host::SshProfile) -> Result<(), NodError>;
        }
    }

    #[test]
    fn daemon_module_exists() {
        let _ = AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        );
    }
}
