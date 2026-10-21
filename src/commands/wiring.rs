//! Presentation-layer composition root (ADR-008).
//!
//! `main` is the only place concrete adapters are wired into the port-holding
//! [`AppContext`]. The application layer (context.rs) holds only the container
//! over `dyn Trait` ports and never imports concrete adapters (ADR-001); this
//! module owns that concrete construction and is invoked from each command arm
//! in `main.rs`.

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::deployment::local_deployer::LocalDeployer;
use crate::infrastructure::deployment::ssh_cli_deployer::SshCliDeployer;
use crate::infrastructure::git::CliGit;
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;
use crate::infrastructure::nix::cli_flake::NixCliFlakeStore;
use crate::infrastructure::provision::NixosAnywhereProvisioner;
use crate::infrastructure::secret::PluggableSecretStore;
use crate::infrastructure::state::{JsonSyncStateStore, SyncLock};
use crate::infrastructure::storage::json_audit_store::JsonAuditStore;

/// composition root: `main` calls it for every command arm and passes the
/// resulting context into `execute`.
///
/// Whereas [`AppContext::new`] takes arbitrary ports (for tests and
/// dependency-free contexts), `production` binds the real evaluator, both
/// deployers, [`TomlConfigStore`] as the `ConfigStorePort`,
/// [`JsonAuditStore`] as the `AuditStorePort`, [`NixCliFlakeStore`] as
/// the `FlakePort`, [`PluggableSecretStore`] as the `SecretPort`, and
/// [`NixosAnywhereProvisioner`] as the `ProvisionerPort`.
pub fn production(flake_path: &Path, cli_overrides: CliOverrides) -> Result<AppContext, NodError> {
    let local = Arc::new(LocalDeployer::new());
    let ssh = Arc::new(SshCliDeployer::new());
    Ok(
        AppContext::new(Arc::new(NixCliEvaluator::new()), local.clone(), ssh.clone())
            .with_config_store(Arc::new(TomlConfigStore::new(flake_path, cli_overrides)?))
            .with_audit_store(Arc::new(JsonAuditStore::from_env()))
            .with_flake_port(Arc::new(NixCliFlakeStore::new()))
            .with_secret_port(Arc::new(PluggableSecretStore::new()))
            .with_provisioner_port(Arc::new(NixosAnywhereProvisioner::new()))
            .with_stores(local, ssh),
    )
}

/// Binds the reconciler ports on top of the production graph (ADR-028).
///
/// The state directory is per invocation because it comes from the resolved
/// sync options rather than from the flake; the composition root still owns
/// the concrete adapters.
pub fn with_sync(ctx: AppContext, state_dir: &Path) -> Result<AppContext, NodError> {
    Ok(ctx
        .with_git_port(Arc::new(CliGit::new()))
        .with_sync_state(Arc::new(JsonSyncStateStore::new(state_dir))))
}

/// Acquires the single-reconciler lock for `state_dir`.
pub fn sync_lock(state_dir: &Path) -> Result<SyncLock, NodError> {
    SyncLock::acquire(state_dir)
}

/// Whether a live reconciler currently holds the lock for `state_dir`.
pub fn sync_lock_held(state_dir: &Path) -> bool {
    SyncLock::held(state_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::CliOverrides;
    use crate::domain::host::{HostEntity, SshProfile};

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
        let ctx = production(dir.path(), CliOverrides::default()).unwrap();
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
}
