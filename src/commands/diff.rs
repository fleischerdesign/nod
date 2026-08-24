//! `nod diff` use case: package & systemd unit diff preview before switching.

use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    flake_path: &Path,
    verbose: bool,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let store = ctx.config_store()?;

    let hosts = evaluator
        .discover_hosts_degraded(flake_path, verbose)
        .await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let targets = resolve_targets(
        hosts,
        &local_hostname,
        target,
        tag,
        role,
        all,
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target.unwrap_or("local"),
            tag,
            role,
        ));
    }

    for host in targets {
        // Materialize merged TOML/CLI user+port onto the entity so the
        // deployer's resolved profile uses them (ADR-004).
        let host = store.apply_to(host).await?;

        println!(
            "{}",
            format!("> Generating system diff preview for {}", host.name)
                .bold()
                .cyan()
        );

        let new_closure = evaluator
            .build_toplevel(flake_path, &host.name, None, verbose)
            .await?;

        let deployer = ctx.deployer_for(&host);
        let profile = ctx.resolved_profile(&host).await?;
        let current_closure = deployer.current_closure(&host, &profile).await?;

        match current_closure {
            Some(current) if current == new_closure => {
                println!(
                    "  {}",
                    "✓ System is already in sync with target closure (no package changes).".green()
                );
            }
            Some(current) => {
                println!(
                    "  {}",
                    format!(
                        "Comparing {} vs {}",
                        current.display(),
                        new_closure.display()
                    )
                    .dimmed()
                );

                let current_str = current.to_str().unwrap_or("");
                let new_str = new_closure.to_str().unwrap_or("");

                let mut rendered = false;
                let nvd_output = Command::new("nvd")
                    .args(["diff", current_str, new_str])
                    .output()
                    .await;

                if let Ok(output) = nvd_output {
                    if output.status.success() {
                        let text = String::from_utf8_lossy(&output.stdout);
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            println!("{trimmed}");
                            rendered = true;
                        }
                    }
                }

                if !rendered {
                    let nix_output = Command::new("nix")
                        .args(["store", "diff-closures", current_str, new_str])
                        .output()
                        .await;

                    match nix_output {
                        Ok(output) if output.status.success() => {
                            let text = String::from_utf8_lossy(&output.stdout);
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                println!("{trimmed}");
                            } else {
                                println!(
                                    "  {}",
                                    "✓ No package version changes detected between closures."
                                        .green()
                                );
                            }
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            println!(
                                "  {}",
                                format!("⚠ Diff tool reported an error: {}", stderr.trim())
                                    .yellow()
                            );
                        }
                        Err(e) => {
                            println!(
                                "  {}",
                                format!("⚠ Failed to launch nix store diff-closures: {e}").yellow()
                            );
                        }
                    }
                }
            }
            None => {
                println!(
                    "  {}",
                    format!(
                        "! No active closure detected on {} (initial deployment). Target: {}",
                        host.name,
                        new_closure.display()
                    )
                    .yellow()
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::{FleetDefaults, HostOverrides};
    use crate::domain::host::{BuilderHost, HostEntity, SshProfile};
    use crate::domain::ports::config_store::ConfigStorePort;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;
    use std::sync::Arc;

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
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    mock! {
        FakeConfigStore {}
        #[async_trait]
        impl ConfigStorePort for FakeConfigStore {
            async fn resolve(&self, host: &HostEntity) -> Result<SshProfile, NodError>;
            async fn host_overrides(&self, name: &str) -> Result<HostOverrides, NodError>;
            async fn fleet_defaults(&self) -> Result<FleetDefaults, NodError>;
            async fn apply_to(&self, host: HostEntity) -> Result<HostEntity, NodError>;
        }
    }

    fn test_ctx(
        eval: MockFakeEvaluator,
        local: MockFakeDeployer,
        ssh: MockFakeDeployer,
        store: MockFakeConfigStore,
    ) -> AppContext {
        AppContext::new(Arc::new(eval), Arc::new(local), Arc::new(ssh))
            .with_config_store(Arc::new(store))
    }

    #[tokio::test]
    async fn diff_rejects_unmatched_target() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("jello", "127.0.0.1", false)]));

        let ctx = test_ctx(
            eval,
            MockFakeDeployer::new(),
            MockFakeDeployer::new(),
            MockFakeConfigStore::new(),
        );

        let err = execute(
            ctx,
            Some("unknown_host"),
            Path::new("."),
            false,
            None,
            None,
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        assert!(err.to_string().contains("no hosts matched"));
    }

    #[tokio::test]
    async fn diff_in_sync_remote_closure_succeeds() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("rollins", "100.126.5.72", false)]));
        eval.expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/test-system-hash")));

        let mut ssh = MockFakeDeployer::new();
        ssh.expect_current_closure()
            .returning(|_, _| Ok(Some(PathBuf::from("/nix/store/test-system-hash"))));

        let mut store = MockFakeConfigStore::new();
        store.expect_apply_to().returning(Ok);
        store
            .expect_resolve()
            .returning(|h| Ok(SshProfile::for_host(h)));

        let ctx = test_ctx(eval, MockFakeDeployer::new(), ssh, store);
        let res = execute(
            ctx,
            Some("rollins"),
            Path::new("."),
            false,
            None,
            None,
            false,
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn diff_missing_active_closure_succeeds() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("rollins", "100.126.5.72", false)]));
        eval.expect_build_toplevel()
            .returning(|_, _, _, _| Ok(PathBuf::from("/nix/store/new-system-hash")));

        let mut ssh = MockFakeDeployer::new();
        ssh.expect_current_closure().returning(|_, _| Ok(None));

        let mut store = MockFakeConfigStore::new();
        store.expect_apply_to().returning(Ok);
        store
            .expect_resolve()
            .returning(|h| Ok(SshProfile::for_host(h)));

        let ctx = test_ctx(eval, MockFakeDeployer::new(), ssh, store);
        let res = execute(
            ctx,
            Some("rollins"),
            Path::new("."),
            false,
            None,
            None,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
