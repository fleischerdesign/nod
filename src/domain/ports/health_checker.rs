//! Health checker port: post-activation verification (ADR-003).

use async_trait::async_trait;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Verifies that a live system is actually healthy after activation.
#[async_trait]
pub trait HealthCheckerPort: Send + Sync {
    /// Runs the health probes for `host`, returning `Ok(true)` when the
    /// system passes verification.
    async fn verify_health(&self, host: &HostEntity) -> Result<bool, NodError>;
}