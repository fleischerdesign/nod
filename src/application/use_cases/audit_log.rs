//! `AuditLogUseCase`: read the deployment history (ADR-003 observability).
//!
//! Composes over `AuditStorePort::entries`; the optional host filter and
//! count cap flow straight through from the CLI.

use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::audit::AuditEntry;
use crate::domain::errors::NodError;

/// Reads the recorded deployment history.
pub struct AuditLogUseCase {
    ctx: Arc<AppContext>,
}

impl AuditLogUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Returns the history, narrowable to `host` and capped at `limit` newest
    /// entries.
    pub async fn execute(
        &self,
        host: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEntry>, NodError> {
        let store = self.ctx.audit_store()?;
        store.entries(host.map(String::from), limit).await
    }
}
