//! `RollbackUseCase`: revert a host to its previous known-good generation
//! profile by dispatching to the resolved `DeployerPort::rollback` (ADR-003).

use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Executes a single-host rollback.
pub struct RollbackUseCase {
    ctx: Arc<AppContext>,
}

impl RollbackUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Rolls `host` back through the deployer its target resolved to.
    pub async fn execute(&self, host: &HostEntity) -> Result<(), NodError> {
        let deployer = self.ctx.deployer_for(host);
        let profile = self.ctx.resolved_profile(host).await?;
        deployer.rollback(host, &profile).await
    }
}
