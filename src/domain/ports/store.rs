//! Domain port for Store and Generation management (ADR-015).

use async_trait::async_trait;
use std::path::Path;

use crate::domain::cache::{CachePushReport, StoreOptimizeReport};
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

    /// Performs hardlink store deduplication (`nix-store --optimise`, ADR-019).
    async fn optimize_store(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
    ) -> Result<StoreOptimizeReport, NodError> {
        let _ = (host, profile);
        Err(NodError::internal(
            "optimize_store not implemented for this adapter",
        ))
    }

    /// Pushes a closure to a remote binary cache (`nix copy --to <cache_uri>`, ADR-019).
    async fn push_cache(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
        closure: &Path,
        cache_uri: &str,
    ) -> Result<CachePushReport, NodError> {
        let _ = (host, profile, closure, cache_uri);
        Err(NodError::internal(
            "push_cache not implemented for this adapter",
        ))
    }
}
