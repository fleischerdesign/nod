//! Config store port: per-host configuration resolution (ADR-004).

use async_trait::async_trait;

use crate::domain::config::{FleetDefaults, HostOverrides};
use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};

/// Supplies resolved configuration for a host.
#[async_trait]
pub trait ConfigStorePort: Send + Sync {
    /// Resolves the effective connection settings for `host`, merging the
    /// CLI / TOML / flake-metadata / defaults tiers (ADR-004).
    async fn resolve(&self, host: &HostEntity) -> Result<SshProfile, NodError>;

    /// Returns the merged per-host overrides (TOML `[hosts.<name>]` over
    /// `[fleet]` over `[defaults]`) for `name`.
    async fn host_overrides(&self, name: &str) -> Result<HostOverrides, NodError>;

    /// Returns the fleet-wide merged defaults (`[fleet]`/`[defaults]` over
    /// the compiled-in defaults).
    async fn fleet_defaults(&self) -> Result<FleetDefaults, NodError>;

    /// Returns `host` with the merged override tiers materialized onto the
    /// entity, so downstream adapters derive the effective profile from it.
    async fn apply_to(&self, host: HostEntity) -> Result<HostEntity, NodError>;
}