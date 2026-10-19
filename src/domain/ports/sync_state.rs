//! Sync state port: the reconciler's memory across cycles (ADR-028).

use async_trait::async_trait;

use crate::domain::errors::NodError;
use crate::domain::sync::SyncState;

/// Persists the reconciler's attempt history.
///
/// The state is *memory*, never the source of truth: the applied state is
/// derived from the live closures. Losing this file costs one drift check,
/// not correctness.
#[async_trait]
pub trait SyncStatePort: Send + Sync {
    /// Loads the stored state, or `None` when the store is empty.
    async fn load(&self) -> Result<Option<SyncState>, NodError>;

    /// Persists `state`, replacing any previous document atomically.
    async fn save(&self, state: &SyncState) -> Result<(), NodError>;
}
