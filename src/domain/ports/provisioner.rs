//! Provisioner port: bare-metal bootstrapping and image generation (ADR-021).

use async_trait::async_trait;
use std::path::Path;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::provision::{BootstrapOptions, BootstrapReport, IsoOptions, IsoReport};

/// Interface for bootstrapping remote machines and generating bootable media.
#[async_trait]
pub trait ProvisionerPort: Send + Sync {
    /// Bootstraps `host` onto a machine reachable at `options.target_ip` via `nixos-anywhere`.
    async fn bootstrap(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        options: &BootstrapOptions,
    ) -> Result<BootstrapReport, NodError>;

    /// Builds a bootable installer ISO or disk image for `host`.
    async fn build_iso(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        options: &IsoOptions,
    ) -> Result<IsoReport, NodError>;
}
