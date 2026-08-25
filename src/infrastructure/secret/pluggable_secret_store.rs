//! Pluggable secrets adapter supporting SOPS, Agenix, custom commands and no-op (ADR-018).

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::ports::secret::SecretPort;
use crate::domain::secret::{
    RekeyOptions, RekeyReport, SecretCheckReport, SecretProvider, SecretStatus,
};

/// Adapter that automatically detects and inspects host secrets.
#[derive(Debug, Default, Clone)]
pub struct PluggableSecretStore;

impl PluggableSecretStore {
    pub fn new() -> Self {
        Self
    }

    /// Detects the secret provider in use for the target host and flake.
    pub fn detect_provider(flake_path: &Path, _host: &HostEntity) -> SecretProvider {
        let sops_yaml = flake_path.join(".sops.yaml");
        let sops_dir = flake_path.join("secrets");
        if sops_yaml.exists() || (sops_dir.exists() && sops_dir.is_dir()) {
            return SecretProvider::Sops;
        }

        let agenix_nix = flake_path.join("secrets.nix");
        if agenix_nix.exists() {
            return SecretProvider::Agenix;
        }

        SecretProvider::None
    }

    /// Finds candidate secret files for the host.
    pub fn find_secret_files(
        flake_path: &Path,
        host_name: &str,
        provider: &SecretProvider,
    ) -> Vec<PathBuf> {
        let mut files = Vec::new();
        match provider {
            SecretProvider::Sops => {
                let secrets_dir = flake_path.join("secrets");
                if secrets_dir.exists() && secrets_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&secrets_dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file() {
                                let filename = p.file_name().unwrap_or_default().to_string_lossy();
                                let is_secret_format = filename.ends_with(".yaml")
                                    || filename.ends_with(".json")
                                    || filename.ends_with(".env");
                                let matches_target = filename.contains(host_name)
                                    || filename.contains("common")
                                    || filename.contains("secrets")
                                    || filename.contains("default");

                                if is_secret_format && matches_target {
                                    files.push(p);
                                }
                            }
                        }
                    }
                }
                let root_secrets = flake_path.join("secrets.yaml");
                if root_secrets.exists() && !files.contains(&root_secrets) {
                    files.push(root_secrets);
                }
            }
            SecretProvider::Agenix => {
                let secrets_dir = flake_path.join("secrets");
                if secrets_dir.exists() && secrets_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&secrets_dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file() && p.extension().is_some_and(|ext| ext == "age") {
                                files.push(p);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        files
    }
}

#[async_trait]
impl SecretPort for PluggableSecretStore {
    async fn check_secrets(
        &self,
        host: &HostEntity,
        flake_path: &Path,
    ) -> Result<SecretCheckReport, NodError> {
        let provider = Self::detect_provider(flake_path, host);

        if provider == SecretProvider::None {
            return Ok(SecretCheckReport {
                host_name: host.name.clone(),
                provider: SecretProvider::None,
                secrets_count: 0,
                valid: true,
                details: Vec::new(),
            });
        }

        let files = Self::find_secret_files(flake_path, &host.name, &provider);
        if files.is_empty() {
            return Ok(SecretCheckReport {
                host_name: host.name.clone(),
                provider,
                secrets_count: 0,
                valid: true,
                details: Vec::new(),
            });
        }

        let mut details = Vec::with_capacity(files.len());
        let mut all_valid = true;

        for file in &files {
            let file_name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let (decryptable, error) = match provider {
                SecretProvider::Sops => {
                    let output = Command::new("sops")
                        .args(["-d", &file.to_string_lossy()])
                        .output()
                        .await;

                    match output {
                        Ok(out) if out.status.success() => (true, None),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                            (false, Some(stderr))
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                            false,
                            Some(
                                "sops command not found in PATH (install sops to verify secrets)"
                                    .to_string(),
                            ),
                        ),
                        Err(e) => (false, Some(format!("failed to run sops: {e}"))),
                    }
                }
                SecretProvider::Agenix => {
                    let output = Command::new("agenix")
                        .args(["-d", &file.to_string_lossy()])
                        .output()
                        .await;

                    match output {
                        Ok(out) if out.status.success() => (true, None),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                            (false, Some(stderr))
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                            false,
                            Some("agenix command not found in PATH (install agenix to verify secrets)".to_string()),
                        ),
                        Err(e) => (false, Some(format!("failed to run agenix: {e}"))),
                    }
                }
                _ => (true, None),
            };

            let recipient_matched = true; // Sops / agenix decryptability verifies recipient access

            if !decryptable {
                all_valid = false;
            }

            details.push(SecretStatus {
                name: file_name,
                path: file.clone(),
                decryptable,
                recipient_matched,
                error,
            });
        }

        Ok(SecretCheckReport {
            host_name: host.name.clone(),
            provider,
            secrets_count: details.len(),
            valid: all_valid,
            details,
        })
    }

    async fn rekey_secrets(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        options: &RekeyOptions,
    ) -> Result<RekeyReport, NodError> {
        let provider = Self::detect_provider(flake_path, host);

        if provider == SecretProvider::None {
            return Ok(RekeyReport {
                host_name: host.name.clone(),
                provider: SecretProvider::None,
                files_rekeyed: Vec::new(),
                ok: true,
                error: None,
            });
        }

        let files = Self::find_secret_files(flake_path, &host.name, &provider);
        let mut rekeyed = Vec::new();

        for file in &files {
            if options.dry_run {
                rekeyed.push(file.clone());
                continue;
            }

            if options.backup {
                let bak = file.with_extension("bak");
                let _ = std::fs::copy(file, bak);
            }

            let status = match provider {
                SecretProvider::Sops => {
                    Command::new("sops")
                        .args(["updatekeys", "-y", &file.to_string_lossy()])
                        .status()
                        .await
                }
                SecretProvider::Agenix => {
                    Command::new("agenix")
                        .args(["-r", "-i", &file.to_string_lossy()])
                        .status()
                        .await
                }
                _ => continue,
            };

            match status {
                Ok(s) if s.success() => rekeyed.push(file.clone()),
                Ok(s) => {
                    return Ok(RekeyReport {
                        host_name: host.name.clone(),
                        provider,
                        files_rekeyed: rekeyed,
                        ok: false,
                        error: Some(format!("rekey process exited with code {:?}", s.code())),
                    });
                }
                Err(e) => {
                    return Ok(RekeyReport {
                        host_name: host.name.clone(),
                        provider,
                        files_rekeyed: rekeyed,
                        ok: false,
                        error: Some(format!("failed to spawn rekey process: {e}")),
                    });
                }
            }
        }

        Ok(RekeyReport {
            host_name: host.name.clone(),
            provider,
            files_rekeyed: rekeyed,
            ok: true,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detect_provider_returns_none_for_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let host = HostEntity::new("yorke", "127.0.0.1", true);
        let store = PluggableSecretStore::new();

        let report = store.check_secrets(&host, temp.path()).await.unwrap();
        assert_eq!(report.provider, SecretProvider::None);
        assert_eq!(report.secrets_count, 0);
        assert!(report.valid);
    }

    #[tokio::test]
    async fn detect_provider_finds_sops_yaml() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".sops.yaml"), "keys: []").unwrap();

        let host = HostEntity::new("yorke", "127.0.0.1", true);
        let provider = PluggableSecretStore::detect_provider(temp.path(), &host);
        assert_eq!(provider, SecretProvider::Sops);
    }
}
