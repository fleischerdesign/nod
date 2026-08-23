//! Deployer port: reachability probing, activation and rollback.

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Deploys (and when requested, rolls back) a host configuration.
///
/// Hosts dispatch to the correct adapter *by target*: the local adapter
/// (`sudo switch-to-configuration`) or the SSH adapter (store copy + remote
/// switch), see ADR-001.
#[async_trait]
pub trait DeployerPort: Send + Sync {
    /// Probes host reachability without using the SSH transport for local
    /// hosts.
    async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;

    /// Resolves the host's currently-active system closure, or `None` when the
    /// host has no live closure attached (drift-detection seam).
    async fn current_closure(&self, host: &HostEntity) -> Result<Option<PathBuf>, NodError>;

    /// Transfers the closure and activates the new system configuration.
    /// `action` is the switch-to-configuration subcommand: `switch`, `test`
    /// or `boot`.
    async fn deploy_and_activate(
        &self,
        host: &HostEntity,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError>;

    /// Reverts a host to its previously known-good generation profile.
    async fn rollback(&self, host: &HostEntity) -> Result<(), NodError>;
}
