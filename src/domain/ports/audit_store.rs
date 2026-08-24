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
}
