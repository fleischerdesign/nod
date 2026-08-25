//! `NixosAnywhereProvisioner`: bare-metal installer and image builder (ADR-021).

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::ports::provisioner::ProvisionerPort;
use crate::domain::provision::{BootstrapOptions, BootstrapReport, IsoOptions, IsoReport};

/// Adapter executing bare-metal installations via `nixos-anywhere` and image builds.
#[derive(Debug, Default, Clone)]
pub struct NixosAnywhereProvisioner;

impl NixosAnywhereProvisioner {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProvisionerPort for NixosAnywhereProvisioner {
    async fn bootstrap(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        options: &BootstrapOptions,
    ) -> Result<BootstrapReport, NodError> {
        let flake_attr = format!("{}#{}", flake_path.to_string_lossy(), host.name);
        let target = format!("{}@{}", options.ssh_user, options.target_ip);

        let mut cmd = Command::new("nixos-anywhere");
        cmd.args(["--flake", &flake_attr]);

        if !options.disko {
            cmd.arg("--no-disko");
        }
        if options.no_kexec {
            cmd.arg("--no-kexec");
        }
        if options.debug {
            cmd.arg("--debug");
        }
        if options.ssh_port != 22 {
            cmd.args(["--ssh-port", &options.ssh_port.to_string()]);
        }
        cmd.arg(&target);

        let output = cmd.output().await;

        match output {
            Ok(out) if out.status.success() => Ok(BootstrapReport {
                host_name: host.name.clone(),
                target_ip: options.target_ip.clone(),
                ok: true,
                error: None,
            }),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Ok(BootstrapReport {
                    host_name: host.name.clone(),
                    target_ip: options.target_ip.clone(),
                    ok: false,
                    error: Some(stderr),
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BootstrapReport {
                host_name: host.name.clone(),
                target_ip: options.target_ip.clone(),
                ok: false,
                error: Some(
                    "nixos-anywhere command not found in PATH (install nixos-anywhere to bootstrap bare-metal nodes)"
                        .to_string(),
                ),
            }),
            Err(e) => Ok(BootstrapReport {
                host_name: host.name.clone(),
                target_ip: options.target_ip.clone(),
                ok: false,
                error: Some(format!("failed to spawn nixos-anywhere: {e}")),
            }),
        }
    }

    async fn build_iso(
        &self,
        host: &HostEntity,
        flake_path: &Path,
        _options: &IsoOptions,
    ) -> Result<IsoReport, NodError> {
        let attr = format!(
            "{}#nixosConfigurations.{}.config.system.build.isoImage",
            flake_path.to_string_lossy(),
            host.name
        );

        let output = Command::new("nix")
            .args(["build", &attr, "--print-out-paths", "--no-link"])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let path = PathBuf::from(stdout);
                Ok(IsoReport {
                    host_name: host.name.clone(),
                    out_path: path,
                    ok: true,
                    error: None,
                })
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Ok(IsoReport {
                    host_name: host.name.clone(),
                    out_path: PathBuf::new(),
                    ok: false,
                    error: Some(stderr),
                })
            }
            Err(e) => Ok(IsoReport {
                host_name: host.name.clone(),
                out_path: PathBuf::new(),
                ok: false,
                error: Some(format!("failed to execute nix build for ISO: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioner_instantiates() {
        let p = NixosAnywhereProvisioner::new();
        let _ = p.clone();
    }
}
