//! Domain entities for Day-0 provisioning, scaffolding, and media generation (ADR-021).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Options controlling bare-metal bootstrapping via `nixos-anywhere`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapOptions {
    /// Target machine IP address or hostname running a live installer environment.
    pub target_ip: String,
    /// SSH username to authenticate against the live installer (default: `root`).
    pub ssh_user: String,
    /// SSH port on the live target (default: 22).
    pub ssh_port: u16,
    /// Whether to run disko partition formatting prior to installation.
    pub disko: bool,
    /// Skip kexec and assume the system is already booted in an installer kernel.
    pub no_kexec: bool,
    /// Enable detailed debug logging during bootstrap execution.
    pub debug: bool,
}

impl Default for BootstrapOptions {
    fn default() -> Self {
        Self {
            target_ip: String::new(),
            ssh_user: "root".to_string(),
            ssh_port: 22,
            disko: true,
            no_kexec: false,
            debug: false,
        }
    }
}

/// Outcome of bare-metal host bootstrapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapReport {
    /// Host name configured.
    pub host_name: String,
    /// Target IP address bootstrapped.
    pub target_ip: String,
    /// Whether bootstrapping completed successfully.
    pub ok: bool,
    /// Error message if bootstrapping failed.
    pub error: Option<String>,
}

/// Template type for repository scaffolding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InitTemplate {
    /// Single-host minimal NixOS flake setup.
    Minimal,
    /// Multi-host fleet configuration with role and tag grouping.
    Fleet,
    /// Production server template with hardened SSH and basic services.
    Server,
}

impl std::str::FromStr for InitTemplate {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "minimal" => Ok(InitTemplate::Minimal),
            "fleet" => Ok(InitTemplate::Fleet),
            "server" => Ok(InitTemplate::Server),
            other => Err(format!(
                "unknown init template: '{other}' (expected: minimal, fleet, server)"
            )),
        }
    }
}

/// Options controlling repository initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitOptions {
    /// Destination directory for the new repository.
    pub target_dir: PathBuf,
    /// Template style to scaffold.
    pub template: InitTemplate,
    /// Descriptive name for the flake / fleet.
    pub flake_name: String,
}

/// Options controlling installation media generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsoOptions {
    /// Output format (e.g. `iso`, `raw-efi`, `qcow2`).
    pub target_format: String,
}

impl Default for IsoOptions {
    fn default() -> Self {
        Self {
            target_format: "iso".to_string(),
        }
    }
}

/// Outcome of ISO/image generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsoReport {
    /// Host name used as configuration base.
    pub host_name: String,
    /// Path to the generated image file.
    pub out_path: PathBuf,
    /// Whether image generation succeeded.
    pub ok: bool,
    /// Error details if build failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_and_iso_reports_serialize_and_deserialize() {
        let b_report = BootstrapReport {
            host_name: "selway".to_string(),
            target_ip: "192.168.1.50".to_string(),
            ok: true,
            error: None,
        };
        let json = serde_json::to_string(&b_report).unwrap();
        let parsed: BootstrapReport = serde_json::from_str(&json).unwrap();
        assert_eq!(b_report, parsed);

        let iso_report = IsoReport {
            host_name: "installer".to_string(),
            out_path: PathBuf::from("/nix/store/test-iso.iso"),
            ok: true,
            error: None,
        };
        let json2 = serde_json::to_string(&iso_report).unwrap();
        let parsed2: IsoReport = serde_json::from_str(&json2).unwrap();
        assert_eq!(iso_report, parsed2);
    }
}
