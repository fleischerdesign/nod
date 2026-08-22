use crate::domain::host::HostEntity;
use anyhow::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

#[async_trait]
pub trait NixEvaluator: Send + Sync {
    async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>>;
    async fn build_toplevel(&self, flake_path: &Path, host_name: &str, verbose: bool) -> Result<PathBuf>;
}
