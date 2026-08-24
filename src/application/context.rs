//! AppContext: dependency-injection container resolving ports for use cases.
//!
//! Presentation seeds the context (ADR-001); Application never imports
//! concrete adapters. Optional services resolve through `Result` so an
//! unregistered binding surfaces as `NodError::config`.

use std::sync::Arc;

use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::ports::audit_store::AuditStorePort;
use crate::domain::ports::config_store::ConfigStorePort;
use crate::domain::ports::deployer::DeployerPort;
use crate::domain::ports::evaluator::EvaluatorPort;
use crate::domain::ports::health_checker::HealthCheckerPort;
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;
use std::path::Path;

/// Resolves every port a use case may need from one seeded container.
pub struct AppContext {
    evaluator: Arc<dyn EvaluatorPort>,
    local_deployer: Arc<dyn DeployerPort>,
    ssh_deployer: Arc<dyn DeployerPort>,
    health_checker: Option<Arc<dyn HealthCheckerPort>>,
    config_store: Option<Arc<dyn ConfigStorePort>>,
    audit_store: Option<Arc<dyn AuditStorePort>>,
}

impl AppContext {
    /// Creates a context with the mandatory ports. Optional services are
    /// left unregistered until configured via `with_*` helpers.
    pub fn new(
        evaluator: Arc<dyn EvaluatorPort>,
        local_deployer: Arc<dyn DeployerPort>,
        ssh_deployer: Arc<dyn DeployerPort>,
    ) -> Self {
        Self {
            evaluator,
            local_deployer,
            ssh_deployer,
            health_checker: None,
            config_store: None,
            audit_store: None,
        }
    }

    /// Builds the complete production graph (ADR-008). This is the single
    /// composition root: `main` calls it for every command arm and passes the
    /// resulting context into `execute`.
    ///
    /// Whereas [`AppContext::new`] takes arbitrary ports (for tests and
    /// dependency-free contexts), `production` binds the real evaluator, both
    /// deployers and a [`TomlConfigStore`] as the `ConfigStorePort`. The audit
    /// store is deliberately NOT bound here: it is an opt-in per-command
    /// binding (`audit` wires it via [`AppContext::with_audit_store`] at a
    /// single call site in `main`).
    pub fn production(flake_path: &Path, cli_overrides: CliOverrides) -> Result<Self, NodError> {
        Ok(Self::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        )
        .with_config_store(Arc::new(TomlConfigStore::new(flake_path, cli_overrides)?)))
    }

    /// Resolves the evaluator port.
    pub fn evaluator(&self) -> Arc<dyn EvaluatorPort> {
        self.evaluator.clone()
    }

    /// Resolves the deployer matching the host's target: local vs SSH.
    pub fn deployer_for(&self, host: &HostEntity) -> Arc<dyn DeployerPort> {
        if host.is_local {
            self.local_deployer.clone()
        } else {
            self.ssh_deployer.clone()
        }
    }

    /// Resolves the *effective* connection profile for `host` (ADR-007, AC1).
    ///
    /// With a bound [`ConfigStorePort`] this is the full four-tier resolved
    /// profile from `config_store.resolve(host)`. Without one it falls back to
    /// [`SshProfile::for_host(host)`] — a deliberate, documented *primitive*
    /// (non-resolved) profile for dependency-free contexts (tests, standalone
    /// `ssh`/`exec` when no config store is wired). The fallback is explicit
    /// and documented, never a silent widening.
    pub async fn resolved_profile(&self, host: &HostEntity) -> Result<SshProfile, NodError> {
        match &self.config_store {
            Some(store) => store.resolve(host).await,
            None => Ok(SshProfile::for_host(host)),
        }
    }

    /// Registers a health checker.
    pub fn with_health_checker(mut self, value: Arc<dyn HealthCheckerPort>) -> Self {
        self.health_checker = Some(value);
        self
    }

    /// Resolves the health checker, or raises a config error when missing.
    pub fn health_checker(&self) -> Result<Arc<dyn HealthCheckerPort>, NodError> {
        self.health_checker
            .clone()
            .ok_or_else(|| NodError::missing_binding("HealthCheckerPort"))
    }

    /// The health checker when one is registered; otherwise `None` (fleet
    /// verification is skipped for hosts whose adapter is absent).
    pub fn health_checker_opt(&self) -> Option<Arc<dyn HealthCheckerPort>> {
        self.health_checker.clone()
    }

    /// Registers a config store.
    pub fn with_config_store(mut self, value: Arc<dyn ConfigStorePort>) -> Self {
        self.config_store = Some(value);
        self
    }

    /// Resolves the config store, or raises a config error when missing.
    pub fn config_store(&self) -> Result<Arc<dyn ConfigStorePort>, NodError> {
        self.config_store
            .clone()
            .ok_or_else(|| NodError::missing_binding("ConfigStorePort"))
    }

    /// Registers an audit store.
    pub fn with_audit_store(mut self, value: Arc<dyn AuditStorePort>) -> Self {
        self.audit_store = Some(value);
        self
    }

    /// Resolves the audit store, or raises a config error when missing.
    pub fn audit_store(&self) -> Result<Arc<dyn AuditStorePort>, NodError> {
        self.audit_store
            .clone()
            .ok_or_else(|| NodError::missing_binding("AuditStorePort"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::{CliOverrides, FleetDefaults, HostOverrides};
    use crate::domain::host::{BuilderHost, SshProfile};
    use crate::infrastructure::deployment::local_deployer::LocalDeployer;
    use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
    use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::{Path, PathBuf};

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

    mock! {
        FakeHealthChecker {}
        #[async_trait]
        impl HealthCheckerPort for FakeHealthChecker {
            async fn verify_health(&self, host: &HostEntity) -> Result<bool, NodError>;
        }
    }

    mock! {
        FakeAuditStore {}
        #[async_trait]
        impl AuditStorePort for FakeAuditStore {
            async fn record(&self, host_name: &str, outcome: &str) -> Result<(), NodError>;
            async fn entries(&self, host: Option<String>, limit: Option<usize>) -> Result<Vec<crate::domain::audit::AuditEntry>, NodError>;
        }
    }

    #[tokio::test]
    async fn production_is_a_single_composition_root_binding_the_config_store() {
        // AC1/AC3: `production` wires the real graph and binds the config
        // store, so `resolved_profile` honours merged overrides instead of
        // falling back to the primitive `SshProfile::for_host`.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".nod.toml"),
            "[hosts.atlas]\nuser = \"philipp\"\n",
        )
        .unwrap();
        let ctx = AppContext::production(dir.path(), CliOverrides::default()).unwrap();
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let profile = ctx.resolved_profile(&host).await.unwrap();
        assert_eq!(
            profile.user(),
            "philipp",
            "production must bind the store so merged overrides apply, \
             not the primitive fallback"
        );
        assert_ne!(profile, SshProfile::for_host(&host));
    }

    #[test]
    fn default_resolution_resolves_distinct_deployers_by_target() {
        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        );
        let local = HostEntity::new("jello", "jello-machine", true);
        let remote = HostEntity::new("atlas", "10.0.0.8", false);
        let local_d = ctx.deployer_for(&local);
        let remote_d = ctx.deployer_for(&remote);
        assert!(
            !Arc::ptr_eq(&local_d, &remote_d),
            "local and ssh deployers must be distinct bindings"
        );
    }

    #[test]
    fn unknown_service_is_a_config_error() {
        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        );
        let err = ctx.config_store().err().unwrap();
        assert!(matches!(err, NodError::Config { .. }));
        assert!(err.to_string().contains("ConfigStorePort"));
    }

    #[tokio::test]
    async fn overriding_a_port_dispatchs_to_the_mock() {
        let mut mock_eval = MockFakeEvaluator::new();
        let hosts = vec![HostEntity::new("atlas", "10.0.0.8", false)];
        mock_eval
            .expect_discover_hosts()
            .times(1)
            .returning(move |_, _| Ok(hosts.clone()));

        let ctx = AppContext::new(
            Arc::new(mock_eval),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        );

        let resolved = ctx
            .evaluator()
            .discover_hosts(Path::new("/tmp/flake"), true)
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "atlas");
    }

    #[tokio::test]
    async fn local_host_probes_through_local_deployer_without_ssh() {
        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        );
        let local = HostEntity::new("jello", "jello-machine", true);
        let is_up = ctx
            .deployer_for(&local)
            .check_reachability(&local)
            .await
            .unwrap();
        assert!(is_up, "local deployer probes reachability without SSH");
    }

    #[tokio::test]
    async fn resolved_profile_without_a_config_store_falls_back_to_primitive() {
        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        );
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let profile = ctx.resolved_profile(&host).await.unwrap();
        assert_eq!(profile, SshProfile::for_host(&host));
    }

    #[tokio::test]
    async fn resolved_profile_with_a_config_store_returns_the_store_resolution() {
        let mut config = MockFakeConfigStore::new();
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let expected = SshProfile::new("philipp", 2200)
            .with_identity_file(std::path::PathBuf::from("/tmp/id_rsa"))
            .with_proxy_jump("bastion");
        let expected_c = expected.clone();
        config
            .expect_resolve()
            .times(1)
            .returning(move |_| Ok(expected_c.clone()));

        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        )
        .with_config_store(Arc::new(config));

        let profile = ctx.resolved_profile(&host).await.unwrap();
        assert_eq!(profile, expected);
    }

    #[tokio::test]
    async fn resolved_profile_feeds_the_shared_argument_builder_exactly() {
        // AC7 regression: the profile that resolved_profile returns (here from
        // a bound store) is exactly the profile the shared build_ssh_args
        // consumes, so identity/proxy/port flow to the SSH transport verbatim.
        let mut config = MockFakeConfigStore::new();
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        config.expect_resolve().times(1).returning(|_| {
            Ok(SshProfile::new("philipp", 2200)
                .with_identity_file(std::path::PathBuf::from("/tmp/id_rsa"))
                .with_proxy_jump("bastion")
                .with_extra_ssh_arg("-o KeepAlive=1".to_string()))
        });

        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        )
        .with_config_store(Arc::new(config));

        let profile = ctx.resolved_profile(&host).await.unwrap();
        let args = crate::domain::ssh_args::build_ssh_args(
            &profile,
            &host.target_host,
            false,
            &["true".to_string()],
        );
        assert_eq!(
            args,
            [
                "-p",
                "2200",
                "-i",
                "/tmp/id_rsa",
                "-J",
                "bastion",
                "-o KeepAlive=1",
                "philipp@10.0.0.8",
                "true"
            ]
        );
    }

    #[tokio::test]
    async fn optional_services_register_and_dispatch_to_their_mocks() {
        let mut config = MockFakeConfigStore::new();
        config
            .expect_resolve()
            .times(1)
            .returning(move |host| Ok(SshProfile::for_host(host)));

        let mut health = MockFakeHealthChecker::new();
        health
            .expect_verify_health()
            .times(1)
            .returning(move |_| Ok(true));

        let mut audit = MockFakeAuditStore::new();
        audit.expect_record().times(1).returning(move |_, _| Ok(()));

        let ctx = AppContext::new(
            Arc::new(NixCliEvaluator::new()),
            Arc::new(LocalDeployer::new()),
            Arc::new(SshCliDeployer::new()),
        )
        .with_config_store(Arc::new(config))
        .with_health_checker(Arc::new(health))
        .with_audit_store(Arc::new(audit));

        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let profile = ctx.config_store().unwrap().resolve(&host).await.unwrap();
        assert_eq!(profile.user(), "root");
        assert!(!profile.sudo());

        let healthy = ctx
            .health_checker()
            .unwrap()
            .verify_health(&host)
            .await
            .unwrap();
        assert!(healthy);

        let recorded = ctx
            .audit_store()
            .unwrap()
            .record(&host.name, "completed")
            .await;
        assert!(recorded.is_ok());
    }
}
