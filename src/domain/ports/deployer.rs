//! Deployer port: reachability probing, activation and rollback.

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};

/// Deploys (and when requested, rolls back) a host configuration.
///
/// Hosts dispatch to the correct adapter *by target*: the local adapter
/// (`sudo switch-to-configuration`) or the SSH adapter (store copy + remote
/// switch), see ADR-001. The caller owns profile resolution: every transport
/// method taking `&SshProfile` receives the *already-resolved* effective
/// connection profile from [`crate::application::context::AppContext::
/// resolved_profile`] (ADR-007); adapters never re-derive a profile
/// themselves and are dumb transports.
#[async_trait]
pub trait DeployerPort: Send + Sync {
    /// Probes host reachability without using the SSH transport for local
    /// hosts. Needs no connection profile: it only pings `target_host`.
    async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;

    /// Resolves the host's currently-active system closure, or `None` when the
    /// host has no live closure attached (drift-detection seam).
    async fn current_closure(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
    ) -> Result<Option<PathBuf>, NodError>;

    /// Transfers the closure and activates the new system configuration.
    /// `action` is the switch-to-configuration subcommand: `switch`, `test`
    /// or `boot`.
    async fn deploy_and_activate(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError>;

    /// Reverts a host to its previously known-good generation profile.
    async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;

    /// Triggers a system reboot on the target host (ADR-017).
    async fn reboot(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError> {
        let _ = (host, profile);
        Err(NodError::internal("reboot not supported for this deployer"))
    }
}
