//! `InspectInfoUseCase`: aggregate static and live diagnostics per host (ADR-016).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::info::HostInfo;
use crate::domain::ssh_args::build_ssh_args;

/// Use case that inspects configuration metadata and live telemetry per host.
pub struct InspectInfoUseCase {
    ctx: Arc<AppContext>,
}

impl InspectInfoUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: &[HostEntity],
        _flake_path: &Path,
    ) -> Result<Vec<HostInfo>, NodError> {
        let mut results = Vec::with_capacity(targets.len());

        for host in targets {
            let profile = self.ctx.resolved_profile(host).await?;
            let deployer = self.ctx.deployer_for(host);

            let is_reachable = deployer.check_reachability(host).await.unwrap_or(false);

            let mut active_generation = None;
            let mut current_closure = None;
            let mut booted_closure = None;
            let mut kernel_version = None;
            let mut uptime = None;
            let mut health_status = None;

            if is_reachable {
                current_closure = deployer
                    .current_closure(host, &profile)
                    .await
                    .ok()
                    .flatten();

                if let Ok(store) = self.ctx.store_for(host) {
                    if let Ok(generations) = store.list_generations(host, &profile).await {
                        active_generation = generations
                            .iter()
                            .find(|g| g.is_current)
                            .map(|g| g.generation);
                    }
                }

                if host.is_local {
                    let booted = Path::new("/run/booted-system");
                    if booted.exists() {
                        booted_closure = std::fs::canonicalize(booted).ok();
                    }

                    if let Ok(output) = Command::new("uname").arg("-r").output().await {
                        if output.status.success() {
                            kernel_version =
                                Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
                        }
                    }

                    if let Ok(output) = Command::new("uptime").arg("-p").output().await {
                        if output.status.success() {
                            uptime =
                                Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
                        }
                    }

                    if let Ok(output) = Command::new("systemctl")
                        .arg("is-system-running")
                        .output()
                        .await
                    {
                        health_status =
                            Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
                    }
                } else {
                    let probe_cmd = "readlink -f /run/booted-system 2>/dev/null || true; echo '---NOD_DELIM---'; uname -r 2>/dev/null || true; echo '---NOD_DELIM---'; uptime -p 2>/dev/null || true; echo '---NOD_DELIM---'; systemctl is-system-running 2>/dev/null || true".to_string();
                    let args = build_ssh_args(&profile, &host.target_host, false, &[probe_cmd]);
                    if let Ok(output) = Command::new("ssh").args(&args).output().await {
                        if output.status.success() {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let parts: Vec<&str> = stdout.split("---NOD_DELIM---").collect();
                            if parts.len() >= 4 {
                                let b = parts[0].trim();
                                if !b.is_empty() {
                                    booted_closure = Some(PathBuf::from(b));
                                }
                                let k = parts[1].trim();
                                if !k.is_empty() {
                                    kernel_version = Some(k.to_string());
                                }
                                let u = parts[2].trim();
                                if !u.is_empty() {
                                    uptime = Some(u.to_string());
                                }
                                let h = parts[3].trim();
                                if !h.is_empty() {
                                    health_status = Some(h.to_string());
                                }
                            }
                        }
                    }
                }
            }

            results.push(HostInfo {
                host_name: host.name.clone(),
                target_host: host.target_host.clone(),
                is_local: host.is_local,
                role: host.role.to_string(),
                tags: host.tags.clone(),
                ssh_user: profile.user().to_string(),
                ssh_port: profile.port(),
                builder: host.nod_config.build.build_host.clone(),
                active_generation,
                current_closure,
                booted_closure,
                kernel_version,
                uptime,
                health_status,
            });
        }

        Ok(results)
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
    async fn inspect_info_assembles_host_info() {
        let mut deployer = MockFakeDeployer::new();
        deployer.expect_check_reachability().returning(|_| Ok(true));
        deployer
            .expect_current_closure()
            .returning(|_, _| Ok(Some(PathBuf::from("/nix/store/test-closure"))));

        let ctx = Arc::new(AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(deployer),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = InspectInfoUseCase::new(ctx);
        let targets = vec![HostEntity::new("yorke", "127.0.0.1", true)];

        let results = use_case.execute(&targets, Path::new(".")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].host_name, "yorke");
        assert_eq!(
            results[0].current_closure,
            Some(PathBuf::from("/nix/store/test-closure"))
        );
    }
}
