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
use crate::infrastructure::nix::cli_evaluator::NixCliEvaluator;

/// Builds the complete production graph (ADR-008). This is the single
/// composition root: `main` calls it for every command arm and passes the
/// resulting context into `execute`.
///
/// Whereas [`AppContext::new`] takes arbitrary ports (for tests and
/// dependency-free contexts), `production` binds the real evaluator, both
/// deployers and a [`TomlConfigStore`] as the `ConfigStorePort`. The audit
/// store is deliberately NOT bound here: it is an opt-in per-command binding
/// (`audit` wires it via [`AppContext::with_audit_store`] at a single call
/// site in `main`).
pub fn production(flake_path: &Path, cli_overrides: CliOverrides) -> Result<AppContext, NodError> {
    Ok(AppContext::new(
        Arc::new(NixCliEvaluator::new()),
        Arc::new(LocalDeployer::new()),
        Arc::new(SshCliDeployer::new()),
    )
    .with_config_store(Arc::new(TomlConfigStore::new(flake_path, cli_overrides)?)))
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
