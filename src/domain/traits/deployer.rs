use crate::domain::host::HostEntity;
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait RemoteDeployer: Send + Sync {
    async fn check_reachability(&self, host: &HostEntity) -> Result<bool>;
    async fn deploy_and_activate(&self, host: &HostEntity, closure: &Path, verbose: bool) -> Result<()>;
    async fn rollback(&self, host: &HostEntity) -> Result<()>;
}
