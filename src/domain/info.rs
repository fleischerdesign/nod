//! Domain entities for host diagnostic inspection (ADR-016).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Comprehensive static and runtime diagnostic information for a fleet host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// Discovered host name.
    pub host_name: String,
    /// Destination IP or host address used for connections.
    pub target_host: String,
    /// True if targeted host is the current local machine.
    pub is_local: bool,
    /// Declared role (e.g. `desktop`, `server`).
    pub role: String,
    /// Declared tags (e.g. `["builder", "workstation"]`).
    pub tags: Vec<String>,
    /// Configured SSH user.
    pub ssh_user: String,
    /// Configured SSH port.
    pub ssh_port: u16,
    /// Configured remote builder assignment, if any.
    pub builder: Option<String>,
    /// Active profile generation number on the host.
    pub active_generation: Option<u32>,
    /// Store path currently activated as `/run/current-system`.
    pub current_closure: Option<PathBuf>,
    /// Store path booted as `/run/booted-system`.
    pub booted_closure: Option<PathBuf>,
    /// Active Linux kernel release string (`uname -r`).
    pub kernel_version: Option<String>,
    /// Formatted host uptime string.
    pub uptime: Option<String>,
    /// Systemd manager operational status (`running`, `degraded`, etc.).
    pub health_status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_serializes_and_deserializes() {
        let info = HostInfo {
            host_name: "yorke".to_string(),
            target_host: "127.0.0.1".to_string(),
            is_local: true,
            role: "desktop".to_string(),
            tags: vec!["workstation".to_string()],
            ssh_user: "philipp".to_string(),
            ssh_port: 22,
            builder: None,
            active_generation: Some(120),
            current_closure: Some(PathBuf::from("/nix/store/test-closure")),
            booted_closure: Some(PathBuf::from("/nix/store/test-closure")),
            kernel_version: Some("6.12.10".to_string()),
            uptime: Some("up 3 days, 4 hours".to_string()),
            health_status: Some("running".to_string()),
        };
        let s = serde_json::to_string(&info).unwrap();
        let parsed: HostInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(info, parsed);
    }
}
