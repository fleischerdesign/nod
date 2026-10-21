//! Audit store port: persistence of per-host deployment outcomes.

use async_trait::async_trait;

use crate::domain::audit::AuditEntry;
use crate::domain::errors::NodError;

/// Persists deployment outcomes for auditability and rollback recovery.
#[async_trait]
pub trait AuditStorePort: Send + Sync {
    /// Records the `outcome` (`completed`, `rolled_back`, ...) for `host_name`.
    async fn record(&self, host_name: &str, outcome: &str) -> Result<(), NodError>;

    /// Reads back the audit history. `host` narrows to a single host and
    /// `limit` caps the count to the newest entries; both are optional (a
    /// missing backing file reads as an empty history, not an error).
    async fn entries(
        &self,
        host: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEntry>, NodError>;

    /// Where this store keeps its history, when it has a single location.
    ///
    /// An empty answer means "no deployment matched", which is not the same
    /// as "no deployment happened": the reconciler writes as its service
    /// account, so an operator reads a different file unless the location is
    /// shared. `nod audit` names the file it read so the two cannot be
    /// confused.
    fn location(&self) -> Option<std::path::PathBuf> {
        None
    }
}
