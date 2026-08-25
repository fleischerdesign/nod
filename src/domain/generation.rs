//! Domain entities for NixOS system generation and store management (ADR-015).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// An individual NixOS profile generation recorded on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemGeneration {
    /// Incremental generation number (e.g. `120`).
    pub generation: u32,
    /// True if this generation is currently active (`/run/current-system`).
    pub is_current: bool,
    /// Epoch timestamp when the generation symlink was created.
    pub created_at: Option<u64>,
    /// Full Nix store path of the generation's toplevel closure.
    pub closure_path: PathBuf,
}

/// Generation history belonging to a specific fleet host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostGenerations {
    pub host_name: String,
    pub generations: Vec<SystemGeneration>,
}

/// Parameters controlling garbage collection on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcOptions {
    /// Age filter string (e.g. `14d`, `30d`).
    pub older_than: Option<String>,
    /// Number of recent generations to preserve.
    pub keep: Option<usize>,
    /// True if GC should only preview freed disk space without deleting.
    pub dry_run: bool,
    /// Concurrency budget for multi-host GC.
    pub concurrency: usize,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            older_than: None,
            keep: None,
            dry_run: false,
            concurrency: 4,
        }
    }
}

/// Outcome of a garbage collection run on a single host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcReport {
    pub host_name: String,
    pub success: bool,
    pub output_summary: String,
}

/// Parameters controlling closure copying to/from a host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyOptions {
    /// Target destination URL (e.g. `ssh://user@host` or S3/HTTP binary cache).
    pub to: Option<String>,
    /// Source URL to copy closures from.
    pub from: Option<String>,
}

/// Outcome of a closure copying operation on a single host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyReport {
    pub host_name: String,
    pub closure_path: PathBuf,
    pub success: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_generation_serialization_round_trip() {
        let gen = SystemGeneration {
            generation: 120,
            is_current: true,
            created_at: Some(1787560497),
            closure_path: PathBuf::from(
                "/nix/store/8rc43cvqzvg01jj10lv2x0h6kwhly52y-nixos-system-yorke",
            ),
        };
        let serialized = serde_json::to_string(&gen).unwrap();
        let deserialized: SystemGeneration = serde_json::from_str(&serialized).unwrap();
        assert_eq!(gen, deserialized);
    }
}
