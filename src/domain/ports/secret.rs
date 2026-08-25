//! Secret port: verification and rekeying of host secrets (ADR-018).

use async_trait::async_trait;
use std::path::Path;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::secret::{RekeyOptions, RekeyReport, SecretCheckReport};

/// Interface for inspecting and rekeying host secrets across pluggable providers.
#[async_trait]
pub trait SecretPort: Send + Sync {
    /// Verifies all declared secrets for `host` (decryptability and recipient rules).
    async fn check_secrets(
        &self,
        host: &HostEntity,
        flake_path: &Path,
    ) -> Result<SecretCheckReport, NodError>;

    /// Re-encrypts secrets for `host` based on updated recipient keys.
    async fn rekey_secrets(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        options: &RekeyOptions,
    ) -> Result<RekeyReport, NodError>;
}
