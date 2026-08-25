//! Evaluator port: Nix host discovery and toplevel closure building.

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::host::{BuilderHost, HostEntity};

/// Discovers `nixosConfigurations` hosts and builds toplevel closures.
#[async_trait]
pub trait EvaluatorPort: Send + Sync {
    /// Discovers all hosts declared in the flake's `nixosConfigurations`.
    async fn discover_hosts(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError>;

    /// Strict discovery (P7a): any per-host metadata `nix eval`/parse failure
    /// is a hard error carrying the failing host's name. Used by targeted
    /// single-host paths so an operator is never silently disconnected from
    /// the one host they meant to touch.
    ///
    /// Provided default delegates to [`Self::discover_hosts`], so non-Nix
    /// evaluators (test mocks) inherit strict behavior without implementing
    /// this method.
    async fn discover_hosts_strict(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        self.discover_hosts(flake_path, verbose).await
    }

    /// Degraded discovery (P7a): a failing host's metadata eval is skipped
    /// and reported via a warning, and the host is omitted from the result;
    /// the rest of the fleet is still discovered. Only a whole-matrix `nix
    /// eval` failure is hard. Used by fleet/all/status paths.
    ///
    /// Provided default delegates to [`Self::discover_hosts`], so non-Nix
    /// evaluators (test mocks) keep working without implementing this method.
    async fn discover_hosts_degraded(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        self.discover_hosts(flake_path, verbose).await
    }

    /// Builds the system toplevel closure for `host_name`, returning the
    /// store path.
    async fn build_toplevel<'a>(
        &self,
        flake_path: &Path,
        host_name: &str,
        builder: Option<&'a BuilderHost>,
        verbose: bool,
    ) -> Result<PathBuf, NodError>;

    /// Evaluates a Nix attribute expression in the context of a host configuration (ADR-016).
    async fn eval_expr(
        &self,
        flake_path: &Path,
        host_name: &str,
        expr: &str,
        json: bool,
    ) -> Result<String, NodError> {
        let _ = (flake_path, host_name, expr, json);
        Err(NodError::internal(
            "eval_expr not implemented for this evaluator",
        ))
    }
}
