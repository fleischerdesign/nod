//! Git port: repository state, remote comparison and fast-forward integration.
//!
//! The reconciler depends on this *shape* only (ADR-001, ADR-028); the CLI
//! adapter in `infrastructure::git` shells out to `git`. Every method is
//! read-only except `fetch` and `fast_forward`, and no method can rewrite
//! history: integration is fast-forward only by contract.

use async_trait::async_trait;
use std::path::Path;

use crate::domain::errors::NodError;

/// Repository inspection and integration operations used by the reconciler.
#[async_trait]
pub trait GitPort: Send + Sync {
    /// `true` when tracked files differ from `HEAD` or the worktree has staged
    /// changes. Untracked files do not count: they are never deployed.
    async fn is_dirty(&self, repo: &Path) -> Result<bool, NodError>;

    /// The commit currently checked out in `repo`.
    async fn local_head(&self, repo: &Path) -> Result<String, NodError>;

    /// The commit `remote/branch` points at, resolved without fetching objects.
    async fn remote_head(
        &self,
        repo: &Path,
        remote: &str,
        branch: &str,
    ) -> Result<String, NodError>;

    /// Fetches `remote/branch` into the local repository.
    async fn fetch(&self, repo: &Path, remote: &str, branch: &str) -> Result<(), NodError>;

    /// Fast-forwards the checked-out branch to `remote/branch` and returns the
    /// new head. A non-fast-forward integration is an error, never a reset.
    async fn fast_forward(
        &self,
        repo: &Path,
        remote: &str,
        branch: &str,
    ) -> Result<String, NodError>;

    /// `true` when `ancestor` is an ancestor of (or equal to) `descendant`.
    async fn is_ancestor(
        &self,
        repo: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, NodError>;

    /// Verifies the commit signature. Returns `false` when the commit is
    /// unsigned or the signature does not verify.
    async fn verify_signature(&self, repo: &Path, rev: &str) -> Result<bool, NodError>;
}
