//! Infrastructure adapter for Nix Flake metadata and lockfile operations (ADR-014).

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::flake::{FlakeInputNode, FlakeMetadata, FlakeUpdateReport, InputDelta};
use crate::domain::ports::flake::FlakePort;

/// Implementation of `FlakePort` using the `nix` CLI and `flake.lock` parsing.
pub struct NixCliFlakeStore;

impl NixCliFlakeStore {
    pub fn new() -> Self {
        Self
    }

    /// Formats an original input descriptor into a human-readable URL string.
    fn format_original_descriptor(original: &serde_json::Value) -> String {
        let input_type = original["type"].as_str().unwrap_or("unknown");
        match input_type {
            "github" | "gitlab" | "sourcehut" => {
                let owner = original["owner"].as_str().unwrap_or("");
                let repo = original["repo"].as_str().unwrap_or("");
                let ref_or_rev = original["ref"]
                    .as_str()
                    .or_else(|| original["rev"].as_str());
                if let Some(r) = ref_or_rev {
                    format!("{input_type}:{owner}/{repo}/{r}")
                } else {
                    format!("{input_type}:{owner}/{repo}")
                }
            }
            "git" | "path" | "tarball" => {
                if let Some(url) = original["url"].as_str() {
                    url.to_string()
                } else if let Some(path) = original["path"].as_str() {
                    format!("path:{path}")
                } else {
                    input_type.to_string()
                }
            }
            _ => {
                if let Some(url) = original["url"].as_str() {
                    url.to_string()
                } else {
                    input_type.to_string()
                }
            }
        }
    }

    /// Parses the JSON output of `nix flake metadata --json` into `FlakeMetadata` and `Vec<FlakeInputNode>`.
    pub fn parse_metadata_json(
        raw_json: &str,
    ) -> Result<(FlakeMetadata, Vec<FlakeInputNode>), NodError> {
        let root_val = serde_json::from_str::<serde_json::Value>(raw_json)
            .map_err(|e| NodError::parse_failure(format!("nix flake metadata JSON: {e}")))?;

        let path = root_val["path"].as_str().unwrap_or("").to_string();
        let url = root_val["url"].as_str().map(|s| s.to_string());
        let revision = root_val["revision"].as_str().map(|s| s.to_string());
        let rev_count = root_val["revCount"].as_u64();
        let last_modified = root_val["lastModified"].as_u64();

        let locks = &root_val["locks"];
        let lock_version = locks["version"].as_u64().unwrap_or(7) as u32;

        let nodes = locks["nodes"]
            .as_object()
            .ok_or_else(|| NodError::parse_failure("missing locks.nodes in flake metadata"))?;

        let root_node = nodes.get("root");
        let direct_inputs_map = root_node
            .and_then(|r| r["inputs"].as_object())
            .cloned()
            .unwrap_or_default();

        let total_inputs = nodes.len().saturating_sub(1);
        let direct_inputs = direct_inputs_map.len();

        let mut input_nodes = Vec::new();

        for (name, node_val) in nodes {
            if name == "root" {
                continue;
            }

            let is_direct = direct_inputs_map.contains_key(name);
            let original_url = Self::format_original_descriptor(&node_val["original"]);
            let locked_rev = node_val["locked"]["rev"].as_str().map(|s| s.to_string());
            let locked_ref = node_val["original"]["ref"]
                .as_str()
                .or_else(|| node_val["locked"]["ref"].as_str())
                .map(|s| s.to_string());
            let node_last_modified = node_val["locked"]["lastModified"].as_u64();
            let nar_hash = node_val["locked"]["narHash"]
                .as_str()
                .map(|s| s.to_string());

            let mut follows = Vec::new();
            if let Some(sub_inputs) = node_val["inputs"].as_object() {
                for (sub_k, sub_v) in sub_inputs {
                    let target_str = if let Some(arr) = sub_v.as_array() {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("/")
                    } else if let Some(s) = sub_v.as_str() {
                        s.to_string()
                    } else {
                        "".to_string()
                    };
                    if !target_str.is_empty() {
                        follows.push((sub_k.clone(), target_str));
                    }
                }
            }

            input_nodes.push(FlakeInputNode {
                name: name.clone(),
                original_url,
                locked_rev,
                locked_ref,
                last_modified: node_last_modified,
                nar_hash,
                follows,
                is_direct,
            });
        }

        // Sort: direct inputs first, then alphabetical by name
        input_nodes.sort_by(|a, b| {
            b.is_direct
                .cmp(&a.is_direct)
                .then_with(|| a.name.cmp(&b.name))
        });

        let metadata = FlakeMetadata {
            path,
            url,
            revision,
            rev_count,
            last_modified,
            lock_version,
            total_inputs,
            direct_inputs,
        };

        Ok((metadata, input_nodes))
    }
}

impl Default for NixCliFlakeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FlakePort for NixCliFlakeStore {
    async fn load_metadata(&self, flake_path: &Path) -> Result<FlakeMetadata, NodError> {
        let output = Command::new("nix")
            .args([
                "flake",
                "metadata",
                "--json",
                flake_path.to_str().unwrap_or("."),
            ])
            .output()
            .await
            .map_err(|e| {
                NodError::evaluation(format!("failed to run `nix flake metadata`: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::evaluation(format!(
                "nix flake metadata failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (metadata, _) = Self::parse_metadata_json(&stdout)?;
        Ok(metadata)
    }

    async fn load_inputs(&self, flake_path: &Path) -> Result<Vec<FlakeInputNode>, NodError> {
        let output = Command::new("nix")
            .args([
                "flake",
                "metadata",
                "--json",
                flake_path.to_str().unwrap_or("."),
            ])
            .output()
            .await
            .map_err(|e| {
                NodError::evaluation(format!("failed to run `nix flake metadata`: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::evaluation(format!(
                "nix flake metadata failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (_, inputs) = Self::parse_metadata_json(&stdout)?;
        Ok(inputs)
    }

    async fn update_inputs(
        &self,
        flake_path: &Path,
        inputs: &[String],
    ) -> Result<FlakeUpdateReport, NodError> {
        // Step 1: Snapshot initial inputs
        let old_inputs = self.load_inputs(flake_path).await?;
        let old_map: HashMap<String, FlakeInputNode> = old_inputs
            .into_iter()
            .map(|node| (node.name.clone(), node))
            .collect();

        // Step 2: Run `nix flake update`
        let mut cmd = Command::new("nix");
        cmd.arg("flake");
        cmd.arg("update");
        for input in inputs {
            cmd.arg(input);
        }
        cmd.arg("--flake");
        cmd.arg(flake_path.to_str().unwrap_or("."));

        let output = cmd
            .output()
            .await
            .map_err(|e| NodError::evaluation(format!("failed to run `nix flake update`: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::evaluation(format!(
                "nix flake update failed: {}",
                stderr.trim()
            )));
        }

        // Step 3: Snapshot new inputs and compute deltas
        let new_inputs = self.load_inputs(flake_path).await?;
        let mut deltas = Vec::new();

        for new_node in new_inputs {
            let old_node = old_map.get(&new_node.name);
            let old_rev = old_node.and_then(|n| n.locked_rev.clone());
            let old_last_modified = old_node.and_then(|n| n.last_modified);

            let delta = InputDelta {
                name: new_node.name,
                old_rev,
                new_rev: new_node.locked_rev,
                old_last_modified,
                new_last_modified: new_node.last_modified,
            };

            if delta.is_changed() {
                deltas.push(delta);
            }
        }

        Ok(FlakeUpdateReport::from_deltas(deltas))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_METADATA_JSON: &str = r#"{
        "locks": {
            "nodes": {
                "nixpkgs": {
                    "locked": {
                        "lastModified": 1787364730,
                        "narHash": "sha256-abc",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "a831408e6378bc02ebf8cc09b52c96ca86f6bab4",
                        "type": "github"
                    },
                    "original": {
                        "owner": "NixOS",
                        "ref": "nixpkgs-unstable",
                        "repo": "nixpkgs",
                        "type": "github"
                    }
                },
                "root": {
                    "inputs": {
                        "nixpkgs": "nixpkgs"
                    }
                }
            },
            "root": "root",
            "version": 7
        },
        "path": "/etc/nixos",
        "revCount": 42,
        "revision": "e22212867d89909a438af05fbd9a9b3c9dbd3d0b",
        "lastModified": 1787600890
    }"#;

    #[test]
    fn parse_metadata_json_extracts_root_and_inputs() {
        let (meta, inputs) = NixCliFlakeStore::parse_metadata_json(SAMPLE_METADATA_JSON).unwrap();
        assert_eq!(meta.path, "/etc/nixos");
        assert_eq!(meta.rev_count, Some(42));
        assert_eq!(meta.lock_version, 7);
        assert_eq!(meta.total_inputs, 1);
        assert_eq!(meta.direct_inputs, 1);

        assert_eq!(inputs.len(), 1);
        let pkgs = &inputs[0];
        assert_eq!(pkgs.name, "nixpkgs");
        assert_eq!(pkgs.original_url, "github:NixOS/nixpkgs/nixpkgs-unstable");
        assert_eq!(pkgs.short_rev(), Some("a831408"));
        assert!(pkgs.is_direct);
    }
}
