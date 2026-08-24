//! Domain entities for Flake metadata and lockfile inspection (ADR-014).

use serde::{Deserialize, Serialize};

/// An input node declared or locked within a flake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlakeInputNode {
    /// Identifier / input key in flake.lock (e.g. `nixpkgs`, `nod`).
    pub name: String,
    /// Original URL or flake reference URI (e.g. `github:NixOS/nixpkgs/nixpkgs-unstable`).
    pub original_url: String,
    /// Locked git revision (full 40-char SHA), if available.
    pub locked_rev: Option<String>,
    /// Locked ref/branch (e.g. `main`, `nixos-unstable`), if applicable.
    pub locked_ref: Option<String>,
    /// Epoch timestamp of last modification.
    pub last_modified: Option<u64>,
    /// NAR hash of the locked source tree.
    pub nar_hash: Option<String>,
    /// Follows or redirected inputs (e.g. `("nixpkgs", "nixpkgs-unstable")`).
    pub follows: Vec<(String, String)>,
    /// True if this node is directly referenced by the root flake.
    pub is_direct: bool,
}

impl FlakeInputNode {
    /// Short 7-character revision hash for display.
    pub fn short_rev(&self) -> Option<&str> {
        self.locked_rev
            .as_deref()
            .map(|r| if r.len() >= 7 { &r[..7] } else { r })
    }
}

/// High-level flake metadata and repository status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlakeMetadata {
    /// Path to the flake source directory.
    pub path: String,
    /// Resolved canonical URL / URI of the flake.
    pub url: Option<String>,
    /// Current git commit hash of the root flake repo.
    pub revision: Option<String>,
    /// Total commit count in the flake git repository.
    pub rev_count: Option<u64>,
    /// Epoch timestamp of last commit or edit.
    pub last_modified: Option<u64>,
    /// Format version of `flake.lock` (typically `7`).
    pub lock_version: u32,
    /// Total number of input nodes in `flake.lock`.
    pub total_inputs: usize,
    /// Total number of direct inputs referenced by root.
    pub direct_inputs: usize,
}

impl FlakeMetadata {
    /// Short 7-character revision hash of the flake repository.
    pub fn short_rev(&self) -> Option<&str> {
        self.revision
            .as_deref()
            .map(|r| if r.len() >= 7 { &r[..7] } else { r })
    }
}

/// Recorded before-and-after difference for an individual flake input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputDelta {
    /// Name of the input node.
    pub name: String,
    /// Previous locked revision.
    pub old_rev: Option<String>,
    /// New locked revision after update.
    pub new_rev: Option<String>,
    /// Previous last modified timestamp.
    pub old_last_modified: Option<u64>,
    /// New last modified timestamp after update.
    pub new_last_modified: Option<u64>,
}

impl InputDelta {
    /// True if revision or timestamp changed.
    pub fn is_changed(&self) -> bool {
        self.old_rev != self.new_rev || self.old_last_modified != self.new_last_modified
    }

    /// Short 7-character old revision.
    pub fn short_old_rev(&self) -> Option<&str> {
        self.old_rev
            .as_deref()
            .map(|r| if r.len() >= 7 { &r[..7] } else { r })
    }

    /// Short 7-character new revision.
    pub fn short_new_rev(&self) -> Option<&str> {
        self.new_rev
            .as_deref()
            .map(|r| if r.len() >= 7 { &r[..7] } else { r })
    }
}

/// Aggregate report of a `nix flake update` run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlakeUpdateReport {
    /// List of inputs that changed.
    pub deltas: Vec<InputDelta>,
    /// Number of updated inputs.
    pub updated_count: usize,
    /// True if no inputs changed.
    pub unchanged: bool,
}

impl FlakeUpdateReport {
    /// Creates a report from calculated deltas.
    pub fn from_deltas(deltas: Vec<InputDelta>) -> Self {
        let changed: Vec<InputDelta> = deltas.into_iter().filter(|d| d.is_changed()).collect();
        let updated_count = changed.len();
        let unchanged = updated_count == 0;
        Self {
            deltas: changed,
            updated_count,
            unchanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_rev_truncates_to_seven_characters() {
        let node = FlakeInputNode {
            name: "nixpkgs".to_string(),
            original_url: "github:NixOS/nixpkgs".to_string(),
            locked_rev: Some("a831408e6378bc02ebf8cc09b52c96ca86f6bab4".to_string()),
            locked_ref: Some("nixpkgs-unstable".to_string()),
            last_modified: Some(1787364730),
            nar_hash: None,
            follows: vec![],
            is_direct: true,
        };
        assert_eq!(node.short_rev(), Some("a831408"));

        let meta = FlakeMetadata {
            path: "/etc/nixos".to_string(),
            url: None,
            revision: Some("e22212867d89909a438af05fbd9a9b3c9dbd3d0b".to_string()),
            rev_count: Some(1663),
            last_modified: Some(1787600890),
            lock_version: 7,
            total_inputs: 15,
            direct_inputs: 8,
        };
        assert_eq!(meta.short_rev(), Some("e222128"));
    }

    #[test]
    fn input_delta_identifies_changes() {
        let delta = InputDelta {
            name: "nod".to_string(),
            old_rev: Some("510c94e24e2c65d213e1f4e58272a3214a4458b6".to_string()),
            new_rev: Some("c1cc7a0e4088d124944ab02cba50d5a801d3fb31".to_string()),
            old_last_modified: Some(1787598405),
            new_last_modified: Some(1787599609),
        };
        assert!(delta.is_changed());
        assert_eq!(delta.short_old_rev(), Some("510c94e"));
        assert_eq!(delta.short_new_rev(), Some("c1cc7a0"));

        let report = FlakeUpdateReport::from_deltas(vec![delta]);
        assert_eq!(report.updated_count, 1);
        assert!(!report.unchanged);
    }
}
