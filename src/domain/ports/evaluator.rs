//! Evaluator port: Nix host discovery and toplevel closure building.

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::host::{BuilderHost, HostEntity};

/// Discovers `nixosConfigurations` hosts and builds toplevel closures.
#[async_trait]
pub trait EvaluatorPort: Send + Sync {
    /// Discovers all hosts declared in the flake's `nixosConfigurations`.
    async fn discover_hosts(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError>;

    /// Builds the system toplevel closure for `host_name`, returning the
    /// store path.
    async fn build_toplevel<'a>(
        &self,
        flake_path: &Path,
        host_name: &str,
        builder: Option<&'a BuilderHost>,
        verbose: bool,
    ) -> Result<PathBuf, NodError>;
}
