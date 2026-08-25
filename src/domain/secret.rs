//! Domain entities for pluggable secrets management (ADR-018).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// The detected or configured secrets management backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretProvider {
    /// sops-nix (SOPS with Age / PGP)
    Sops,
    /// agenix (Age encryption)
    Agenix,
    /// Custom operator-defined check/rekey command
    Custom(String),
    /// No secrets management declared on target
    None,
}

impl fmt::Display for SecretProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretProvider::Sops => write!(f, "sops"),
            SecretProvider::Agenix => write!(f, "agenix"),
            SecretProvider::Custom(cmd) => write!(f, "custom ({cmd})"),
            SecretProvider::None => write!(f, "none"),
        }
    }
}

/// Verification status for a single secret item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretStatus {
    /// Secret name or identifier.
    pub name: String,
    /// Path to the encrypted secret file.
    pub path: PathBuf,
    /// Whether the file was successfully decrypted.
    pub decryptable: bool,
    /// Whether the target host's public key is present in recipient rules.
    pub recipient_matched: bool,
    /// Descriptive error message if verification failed.
    pub error: Option<String>,
}

/// Aggregated secret verification report for a single host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretCheckReport {
    /// Target host name.
    pub host_name: String,
    /// Secrets provider in use.
    pub provider: SecretProvider,
    /// Total number of secrets examined.
    pub secrets_count: usize,
    /// True if all secrets are valid and decryptable.
    pub valid: bool,
    /// Detailed status per secret file.
    pub details: Vec<SecretStatus>,
}

/// Options controlling the secret rekey operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RekeyOptions {
    /// Preview rekey actions without modifying files on disk.
    pub dry_run: bool,
    /// Create `.bak` backup copies before rekeying.
    pub backup: bool,
}

impl Default for RekeyOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            backup: true,
        }
    }
}

/// Outcome of rekeying secrets for a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RekeyReport {
    /// Target host name.
    pub host_name: String,
    /// Secrets provider in use.
    pub provider: SecretProvider,
    /// List of file paths rekeyed.
    pub files_rekeyed: Vec<PathBuf>,
    /// Whether rekeying succeeded.
    pub ok: bool,
    /// Error message if rekeying failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_report_serializes_and_deserializes() {
        let report = SecretCheckReport {
            host_name: "yorke".to_string(),
            provider: SecretProvider::Sops,
            secrets_count: 1,
            valid: true,
            details: vec![SecretStatus {
                name: "wireguard-key".to_string(),
                path: PathBuf::from("secrets/wireguard.yaml"),
                decryptable: true,
                recipient_matched: true,
                error: None,
            }],
        };

        let json = serde_json::to_string(&report).unwrap();
        let parsed: SecretCheckReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, parsed);
    }
}
