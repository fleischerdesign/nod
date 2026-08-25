//! Domain port for Store and Generation management (ADR-015).

use async_trait::async_trait;
use std::path::Path;

use crate::domain::errors::NodError;
use crate::domain::generation::{CopyOptions, CopyReport, GcOptions, GcReport, SystemGeneration};
use crate::domain::host::{HostEntity, SshProfile};

/// SPI port for inspecting generations, collecting garbage, and copying closures.
#[async_trait]
pub trait StorePort: Send + Sync {
    /// Lists all available profile generations on the targeted host.
    async fn list_generations(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
    ) -> Result<Vec<SystemGeneration>, NodError>;

    /// Executes garbage collection on the targeted host.
    async fn collect_garbage(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
        options: &GcOptions,
    ) -> Result<GcReport, NodError>;

    /// Copies a store closure to or from the targeted host.
    async fn copy_closure(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
        closure: &Path,
        options: &CopyOptions,
    ) -> Result<CopyReport, NodError>;
}
