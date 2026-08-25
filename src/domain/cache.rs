//! Domain entities for store optimization and binary cache integration (ADR-019).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Outcome of hardlink store deduplication (`nix-store --optimise`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreOptimizeReport {
    /// Target host name.
    pub host_name: String,
    /// Whether optimization succeeded.
    pub ok: bool,
    /// Approximate bytes freed through deduplication, if available.
    pub freed_bytes: Option<u64>,
    /// Error message if optimization failed.
    pub error: Option<String>,
}

/// Options controlling closure pushing to a binary cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePushOptions {
    /// Destination binary cache URI (e.g. `s3://cache`, `ssh://cache`, `https://cache.example.com`).
    pub cache_uri: Option<String>,
    /// Preview push operations without actually transferring data.
    pub dry_run: bool,
    /// Maximum concurrent cache pushes.
    pub concurrency: usize,
}

impl Default for CachePushOptions {
    fn default() -> Self {
        Self {
            cache_uri: None,
            dry_run: false,
            concurrency: 4,
        }
    }
}

/// Outcome of pushing a system closure to a binary cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePushReport {
    /// Target host name.
    pub host_name: String,
    /// Store path that was pushed.
    pub closure_path: PathBuf,
    /// Binary cache destination URI.
    pub cache_uri: String,
    /// Whether push completed successfully.
    pub ok: bool,
    /// Error details if push failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_reports_serialize_and_deserialize() {
        let opt_report = StoreOptimizeReport {
            host_name: "yorke".to_string(),
            ok: true,
            freed_bytes: Some(1048576),
            error: None,
        };
        let s = serde_json::to_string(&opt_report).unwrap();
        let parsed: StoreOptimizeReport = serde_json::from_str(&s).unwrap();
        assert_eq!(opt_report, parsed);

        let push_report = CachePushReport {
            host_name: "yorke".to_string(),
            closure_path: PathBuf::from("/nix/store/test-closure"),
            cache_uri: "s3://my-cache".to_string(),
            ok: true,
            error: None,
        };
        let s2 = serde_json::to_string(&push_report).unwrap();
        let parsed2: CachePushReport = serde_json::from_str(&s2).unwrap();
        assert_eq!(push_report, parsed2);
    }
}
