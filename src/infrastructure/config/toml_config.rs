//! TOML config store adapter: `.nod.toml` host overrides and fleet defaults.
//!
//! Implements ADR-004's four-tier precedence for per-host connection options,
//! with the highest tier winning per option:
//!   1. CLI overrides (seeded at construction),
//!   2. Local `.nod.toml` (`[hosts.<name>]` > `[fleet]` > `[defaults]`),
//!   3. flake metadata (already materialized on the `HostEntity`),
//!   4. compiled-in defaults (`root` / `22`, carried by `HostEntity::new`).
//!
//! No module outside this adapter reads `.nod.toml` (ADR-004 compliance).

use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::domain::config::{CliOverrides, FleetDefaults, HostOverrides};
use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::ports::config_store::ConfigStorePort;

/// File names searched bottom-up from the flake root / cwd.
const CONFIG_FILE_NAMES: [&str; 2] = [".nod.toml", "nod.toml"];

/// SSH settings shared by the `[defaults]` and `[fleet]` sections.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SshOverrides {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub sudo: Option<bool>,
}

/// A `[hosts.<name>]` section: the shared SSH settings plus host-specific
/// topology values (`target_host`, `role`, `tags`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlHost {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub sudo: Option<bool>,
    pub target_host: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Parsed shape of `.nod.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlConfig {
    #[serde(default)]
    pub defaults: Option<SshOverrides>,
    #[serde(default)]
    pub fleet: Option<SshOverrides>,
    #[serde(default)]
    pub hosts: HashMap<String, TomlHost>,
}

/// Fully merged overrides for one host across the TOML and CLI tiers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Merged {
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<PathBuf>,
    proxy_jump: Option<String>,
    sudo: Option<bool>,
}

impl Merged {
    /// Overlays one section's SSH values (later calls win).
    fn overlay(
        &mut self,
        user: Option<String>,
        port: Option<u16>,
        identity_file: Option<PathBuf>,
        proxy_jump: Option<String>,
        sudo: Option<bool>,
    ) {
        if user.is_some() {
            self.user = user;
        }
        if port.is_some() {
            self.port = port;
        }
        if identity_file.is_some() {
            self.identity_file = identity_file;
        }
        if proxy_jump.is_some() {
            self.proxy_jump = proxy_jump;
        }
        if sudo.is_some() {
            self.sudo = sudo;
        }
    }
}

/// Overlays the CLI tier (always wins) onto a merge.
fn merge_cli_into(merged: &mut Merged, cli: &CliOverrides) {
    if cli.user.is_some() {
        merged.user = cli.user.clone();
    }
    if cli.port.is_some() {
        merged.port = cli.port;
    }
    if cli.identity_file.is_some() {
        merged.identity_file = cli.identity_file.clone();
    }
}

/// Config store backed by a local `.nod.toml` discovered bottom-up from the
/// flake root.
pub struct TomlConfigStore {
    cli: CliOverrides,
    toml: TomlConfig,
}

impl TomlConfigStore {
    /// Builds the store for `root`, discovering `.nod.toml` (or `nod.toml`)
    /// by walking up from `root`. Applies `cli` overrides as the top tier.
    pub fn new(root: &Path, cli: CliOverrides) -> Result<Self, NodError> {
        let toml = match Self::discover(root) {
            Some(path) => Self::parse(&path)?,
            None => TomlConfig::default(),
        };
        Ok(Self {
            cli,
            toml,
        })
    }

    /// Finds the nearest config file at or above `start`.
    fn discover(start: &Path) -> Option<PathBuf> {
        let mut dir: Option<&Path> = Some(start);
        while let Some(d) = dir {
            for name in CONFIG_FILE_NAMES {
                let candidate = d.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            dir = d.parent();
        }
        None
    }

    /// Parses the config file, surfacing malformed TOML as `NodError::config`.
    fn parse(path: &Path) -> Result<TomlConfig, NodError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| NodError::config_parse(format!("cannot read {}: {}", path.display(), e)))?;
        toml::from_str::<TomlConfig>(&raw)
            .map_err(|e| NodError::config_parse(format!("invalid {}: {}", path.display(), e)))
    }

    /// Merges the `[defaults]` and `[fleet]` sections (no CLI tier).
    fn base_merged(&self) -> Merged {
        let mut merged = Merged::default();
        if let Some(d) = &self.toml.defaults {
            merged.overlay(
                d.user.clone(),
                d.port,
                d.identity_file.clone(),
                d.proxy_jump.clone(),
                d.sudo,
            );
        }
        if let Some(f) = &self.toml.fleet {
            merged.overlay(
                f.user.clone(),
                f.port,
                f.identity_file.clone(),
                f.proxy_jump.clone(),
                f.sudo,
            );
        }
        merged
    }

    /// Merges every tier except the host entity itself (tiers 3/4 live on
    /// the entity) into one override set for `name`.
    fn merged_for(&self, name: &str) -> Merged {
        let mut merged = self.base_merged();
        if let Some(h) = self.toml.hosts.get(name) {
            merged.overlay(
                h.user.clone(),
                h.port,
                h.identity_file.clone(),
                h.proxy_jump.clone(),
                h.sudo,
            );
        }
        merge_cli_into(&mut merged, &self.cli);
        merged
    }
}

#[async_trait]
impl ConfigStorePort for TomlConfigStore {
    async fn resolve(&self, host: &HostEntity) -> Result<SshProfile, NodError> {
        let merged = self.merged_for(&host.name);
        let mut profile = SshProfile::for_host(host);
        if let Some(user) = merged.user {
            profile = profile.with_user(user);
        }
        if let Some(port) = merged.port {
            profile = profile.with_port(port);
        }
        if let Some(identity) = merged.identity_file {
            profile = profile.with_identity_file(identity);
        }
        if let Some(proxy) = merged.proxy_jump {
            profile = profile.with_proxy_jump(proxy);
        }
        if let Some(sudo) = merged.sudo {
            profile = profile.with_sudo(sudo);
        }
        Ok(profile)
    }

    async fn host_overrides(&self, name: &str) -> Result<HostOverrides, NodError> {
        let merged = self.merged_for(name);
        let host = self.toml.hosts.get(name);
        Ok(HostOverrides {
            target_host: host.and_then(|h| h.target_host.clone()),
            user: merged.user,
            port: merged.port,
            identity_file: merged.identity_file,
            proxy_jump: merged.proxy_jump,
            sudo: merged.sudo,
            role: host.and_then(|h| h.role.clone()),
            tags: host.and_then(|h| h.tags.clone()),
        })
    }

    async fn fleet_defaults(&self) -> Result<FleetDefaults, NodError> {
        let merged = self.base_merged();
        // CLI overrides are per-run, not fleet defaults.
        Ok(FleetDefaults {
            user: merged.user,
            port: merged.port,
            identity_file: merged.identity_file,
            proxy_jump: merged.proxy_jump,
            sudo: merged.sudo,
        })
    }

    async fn apply_to(&self, mut host: HostEntity) -> Result<HostEntity, NodError> {
        let profile = self.resolve(&host).await?;
        host.target_user = profile.user().to_string();
        host.target_port = profile.port();
        Ok(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_toml(root: &Path, contents: &str) {
        std::fs::write(root.join(".nod.toml"), contents).unwrap();
    }

    fn store(root: &Path, cli: CliOverrides) -> TomlConfigStore {
        TomlConfigStore::new(root, cli).unwrap()
    }

    fn host(name: &str) -> HostEntity {
        HostEntity::new(name, "10.0.0.8", false)
    }

    #[tokio::test]
    async fn missing_config_falls_back_to_compiled_defaults() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let s = store(root, CliOverrides::default());
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "root");
        assert_eq!(profile.port(), 22);
        assert_eq!(profile.identity_file(), None);
        assert_eq!(profile.proxy_jump(), None);
        assert!(!profile.sudo());
    }

    #[tokio::test]
    async fn defaults_section_underpins_every_host() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[defaults]\nuser = \"deploy\"\nport = 2200\n");
        let s = store(root, CliOverrides::default());
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "deploy");
        assert_eq!(profile.port(), 2200);
    }

    #[tokio::test]
    async fn fleet_section_overrides_defaults() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[defaults]\nuser = \"deploy\"\n[fleet]\nuser = \"fleet\"\n");
        let s = store(root, CliOverrides::default());
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "fleet");
    }

    #[tokio::test]
    async fn host_section_overrides_fleet_and_defaults() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(
            root,
            "[defaults]\nuser = \"deploy\"\n[fleet]\nport = 2200\n[hosts.atlas]\nuser = \"philipp\"\nport = 2222\n",
        );
        let s = store(root, CliOverrides::default());

        let atlas = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(atlas.user(), "philipp");
        assert_eq!(atlas.port(), 2222);

        let orbit = s.resolve(&host("orbit")).await.unwrap();
        assert_eq!(orbit.user(), "deploy");
        assert_eq!(orbit.port(), 2200);
    }

    #[tokio::test]
    async fn cli_overrides_beat_host_section() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[hosts.atlas]\nuser = \"toml\"\n");
        let cli = CliOverrides { user: Some("cli-user".to_string()), port: None, identity_file: None };
        let s = store(root, cli);
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "cli-user");
    }

    #[tokio::test]
    async fn cli_port_overrides_toml_section() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[hosts.atlas]\nport = 2222\n");
        let cli = CliOverrides { user: None, port: Some(2200), identity_file: None };
        let s = store(root, cli);
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "root");
        assert_eq!(profile.port(), 2200);
    }

    #[tokio::test]
    async fn identity_proxy_and_sudo_flow_through_tiers() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(
            root,
            "[fleet]\nidentity_file = \"/var/lib/nod/id\"\nproxy_jump = \"bastion.example.org\"\n\n[hosts.atlas]\nsudo = true\n",
        );
        let s = store(root, CliOverrides::default());

        let atlas = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(atlas.identity_file(), Some(PathBuf::from("/var/lib/nod/id")).as_ref());
        assert_eq!(atlas.proxy_jump(), Some("bastion.example.org"));
        assert!(atlas.sudo());

        // A host without its own section inherits the fleet identity file.
        let orbit = s.resolve(&host("orbit")).await.unwrap();
        assert_eq!(orbit.identity_file(), Some(PathBuf::from("/var/lib/nod/id")).as_ref());
    }

    #[tokio::test]
    async fn host_overrides_exposes_merged_ssh_and_topology_values() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(
            root,
            "[fleet]\nuser = \"deploy\"\n[hosts.atlas]\nuser = \"philipp\"\nrole = \"server\"\ntags = [\"prod\", \"web\"]\n",
        );
        let s = store(root, CliOverrides::default());
        let overrides = s.host_overrides("atlas").await.unwrap();
        assert_eq!(overrides.user, Some("philipp".to_string()));
        assert_eq!(overrides.role, Some("server".to_string()));
        assert_eq!(overrides.tags, Some(vec!["prod".to_string(), "web".to_string()]));
    }

    #[tokio::test]
    async fn fleet_defaults_omit_cli_overrides() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[fleet]\nuser = \"fleet\"\nport = 2222\n");
        let cli = CliOverrides { user: Some("cli".to_string()), port: Some(99), identity_file: None };
        let s = store(root, cli);
        let defaults = s.fleet_defaults().await.unwrap();
        assert_eq!(defaults.user, Some("fleet".to_string()));
        assert_eq!(defaults.port, Some(2222));
    }

    #[tokio::test]
    async fn apply_to_materializes_user_and_port_but_keeps_locality() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[hosts.atlas]\nuser = \"philipp\"\nport = 2222\n");
        let s = store(root, CliOverrides::default());
        let h = host("atlas");
        let materialized = s.apply_to(h.clone()).await.unwrap();
        assert_eq!(materialized.target_user, "philipp");
        assert_eq!(materialized.target_port, 2222);
        assert!(!materialized.is_local);
    }

    #[tokio::test]
    async fn invalid_toml_raises_config_error() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "this is not = [valid toml");
        let err = TomlConfigStore::new(root, CliOverrides::default()).err().unwrap();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[tokio::test]
    async fn discovery_walks_up_from_a_subdirectory() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[hosts.atlas]\nuser = \"philipp\"\n");
        let sub = root.join("deep").join("nested");
        std::fs::create_dir_all(&sub).unwrap();

        let s = store(&sub, CliOverrides::default());
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "philipp");
    }
}