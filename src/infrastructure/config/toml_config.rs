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

use crate::domain::config::{
    BuildConfig, CliOverrides, CustomProbeConfig, FleetDefaults, HealthCheckConfig, HooksConfig,
    HostOverrides, HttpProbeConfig, RolloutConfig, SshConnectionOverrides, SystemdHealthConfig,
};
use crate::domain::errors::NodError;
use crate::domain::host::{HostEntity, SshProfile};
use crate::domain::ports::config_store::ConfigStorePort;

/// File names searched bottom-up from the flake root / cwd.
const CONFIG_FILE_NAMES: [&str; 2] = [".nod.toml", "nod.toml"];

/// SSH settings shared by the `[defaults]`, `[fleet]` and nested
/// `[hosts.<name>.ssh]` sections. Fields are optional so an absent section
/// leaves the tier unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SshOverrides {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub sudo: Option<bool>,
    pub timeout_secs: Option<u32>,
    pub connect_timeout_secs: Option<u32>,
    #[serde(default)]
    pub extra_ssh_args: Option<Vec<String>>,
    pub allow_insecure: Option<bool>,
}

/// A `[hosts.<name>]` section: shared SSH settings plus host-specific
/// topology values (`target_host`, `role`, `tags`, `description`) and the
/// granular group tables (`ssh`, `build`, `rollout`, `health_checks`, `hooks`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlHost {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub sudo: Option<bool>,
    pub timeout_secs: Option<u32>,
    pub connect_timeout_secs: Option<u32>,
    #[serde(default)]
    pub extra_ssh_args: Option<Vec<String>>,
    pub allow_insecure: Option<bool>,
    pub target_host: Option<String>,
    pub role: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub ssh: Option<SshOverrides>,
    #[serde(default)]
    pub build: Option<TomlBuild>,
    #[serde(default)]
    pub rollout: Option<TomlRollout>,
    #[serde(default)]
    pub health_checks: Option<TomlHealth>,
    #[serde(default)]
    pub hooks: Option<TomlHooks>,
}

/// `[hosts.<name>.build]` — 1:1 with `options.nod.build`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlBuild {
    pub build_host: Option<String>,
    #[serde(default)]
    pub substituters: Option<Vec<String>>,
    #[serde(default)]
    pub trusted_public_keys: Option<Vec<String>>,
    #[serde(default)]
    pub eval_flags: Option<Vec<String>>,
    pub show_trace: Option<bool>,
    pub impure: Option<bool>,
}

/// `[hosts.<name>.rollout]` — 1:1 with `options.nod.rollout`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlRollout {
    pub priority: Option<u32>,
    pub action: Option<String>,
    pub auto_rollback: Option<bool>,
    pub reboot: Option<bool>,
    pub magic_rollback: Option<bool>,
    pub magic_rollback_timeout_secs: Option<u32>,
}

/// `[hosts.<name>.health_checks.systemd]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlSystemd {
    pub check_running: Option<bool>,
    pub check_failed_units: Option<bool>,
    #[serde(default)]
    pub required_units: Option<Vec<String>>,
}

/// One `[hosts.<name>.health_checks.http_probes]` entry.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlHttp {
    pub url: Option<String>,
    pub expected_status: Option<u16>,
    pub timeout_secs: Option<u32>,
}

/// One `[hosts.<name>.health_checks.custom_probes]` entry.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlCustom {
    pub name: Option<String>,
    pub command: Option<String>,
    pub timeout_secs: Option<u32>,
}

/// `[hosts.<name>.health_checks]` — 1:1 with `options.nod.healthChecks`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlHealth {
    pub enable: Option<bool>,
    pub timeout_secs: Option<u32>,
    #[serde(default)]
    pub systemd: Option<TomlSystemd>,
    #[serde(default)]
    pub tcp_ports: Option<Vec<u16>>,
    #[serde(default)]
    pub http_probes: Option<Vec<TomlHttp>>,
    #[serde(default)]
    pub custom_probes: Option<Vec<TomlCustom>>,
}

/// `[hosts.<name>.hooks]` — 1:1 with `options.nod.hooks`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlHooks {
    pub pre_switch_hook: Option<String>,
    pub post_switch_hook: Option<String>,
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

/// Fully merged SSH overrides for one host across the TOML and CLI tiers.
/// `ssh` carries every granular `options.nod.ssh` value after cascading
/// defaults → fleet → host → CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Merged {
    ssh: SshOverrides,
}

impl Merged {
    /// Overlays one source's SSH values onto the merge (later calls win).
    fn overlay(&mut self, src: &SshOverrides) {
        if src.user.is_some() {
            self.ssh.user = src.user.clone();
        }
        if src.port.is_some() {
            self.ssh.port = src.port;
        }
        if src.identity_file.is_some() {
            self.ssh.identity_file = src.identity_file.clone();
        }
        if src.proxy_jump.is_some() {
            self.ssh.proxy_jump = src.proxy_jump.clone();
        }
        if src.proxy_command.is_some() {
            self.ssh.proxy_command = src.proxy_command.clone();
        }
        if src.sudo.is_some() {
            self.ssh.sudo = src.sudo;
        }
        if src.timeout_secs.is_some() {
            self.ssh.timeout_secs = src.timeout_secs;
        }
        if src.connect_timeout_secs.is_some() {
            self.ssh.connect_timeout_secs = src.connect_timeout_secs;
        }
        if src.extra_ssh_args.is_some() {
            self.ssh.extra_ssh_args = src.extra_ssh_args.clone();
        }
        if src.allow_insecure.is_some() {
            self.ssh.allow_insecure = src.allow_insecure;
        }
    }
}

/// Overlays the CLI tier (always wins) onto a merge.
fn merge_cli_into(merged: &mut Merged, cli: &CliOverrides) {
    if cli.user.is_some() {
        merged.ssh.user = cli.user.clone();
    }
    if cli.port.is_some() {
        merged.ssh.port = cli.port;
    }
    if cli.identity_file.is_some() {
        merged.ssh.identity_file = cli.identity_file.clone();
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
        Ok(Self { cli, toml })
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
        let raw = std::fs::read_to_string(path).map_err(|e| {
            NodError::config_parse(format!("cannot read {}: {}", path.display(), e))
        })?;
        toml::from_str::<TomlConfig>(&raw)
            .map_err(|e| NodError::config_parse(format!("invalid {}: {}", path.display(), e)))
    }

    /// Merges the `[defaults]` and `[fleet]` sections (no CLI tier).
    fn base_merged(&self) -> Merged {
        let mut merged = Merged::default();
        if let Some(d) = &self.toml.defaults {
            merged.overlay(d);
        }
        if let Some(f) = &self.toml.fleet {
            merged.overlay(f);
        }
        merged
    }

    /// Merges every tier except the host entity itself (tiers 3/4 live on
    /// the entity) into one override set for `name`. Flat `[hosts.<name>]`
    /// ssh values are overlaid before the nested `[hosts.<name>.ssh]` table,
    /// then the CLI tier wins.
    fn merged_for(&self, name: &str) -> Merged {
        let mut merged = self.base_merged();
        if let Some(h) = self.toml.hosts.get(name) {
            let flat = SshOverrides {
                user: h.user.clone(),
                port: h.port,
                identity_file: h.identity_file.clone(),
                proxy_jump: h.proxy_jump.clone(),
                proxy_command: h.proxy_command.clone(),
                sudo: h.sudo,
                timeout_secs: h.timeout_secs,
                connect_timeout_secs: h.connect_timeout_secs,
                extra_ssh_args: h.extra_ssh_args.clone(),
                allow_insecure: h.allow_insecure,
            };
            merged.overlay(&flat);
            if let Some(ssh) = &h.ssh {
                merged.overlay(ssh);
            }
        }
        merge_cli_into(&mut merged, &self.cli);
        merged
    }
}

/// Maps a parsed `[hosts.<name>.build]` table onto the domain `BuildConfig`.
fn to_build_config(b: &TomlBuild) -> BuildConfig {
    BuildConfig {
        build_host: b.build_host.clone(),
        substituters: b.substituters.clone(),
        trusted_public_keys: b.trusted_public_keys.clone(),
        eval_flags: b.eval_flags.clone(),
        show_trace: b.show_trace,
        impure: b.impure,
    }
}

/// Maps a parsed `[hosts.<name>.rollout]` table onto the domain `RolloutConfig`.
fn to_rollout_config(r: &TomlRollout) -> RolloutConfig {
    RolloutConfig {
        priority: r.priority,
        action: r.action.clone(),
        auto_rollback: r.auto_rollback,
        reboot: r.reboot,
        magic_rollback: r.magic_rollback,
        magic_rollback_timeout_secs: r.magic_rollback_timeout_secs,
    }
}

/// Maps a parsed `[hosts.<name>.hooks]` table onto the domain `HooksConfig`.
fn to_hooks_config(h: &TomlHooks) -> HooksConfig {
    HooksConfig {
        pre_switch_hook: h.pre_switch_hook.clone(),
        post_switch_hook: h.post_switch_hook.clone(),
    }
}

/// Maps a parsed `[hosts.<name>.health_checks]` table onto the domain
/// `HealthCheckConfig`.
fn to_health_config(h: &TomlHealth) -> HealthCheckConfig {
    let systemd = match &h.systemd {
        Some(s) => SystemdHealthConfig {
            check_running: s.check_running,
            check_failed_units: s.check_failed_units,
            required_units: s.required_units.clone(),
        },
        None => SystemdHealthConfig::default(),
    };
    let http_probes = h.http_probes.as_ref().map(|probes| {
        probes
            .iter()
            .map(|p| HttpProbeConfig {
                url: p.url.clone(),
                expected_status: p.expected_status,
                timeout_secs: p.timeout_secs,
            })
            .collect()
    });
    let custom_probes = h.custom_probes.as_ref().map(|probes| {
        probes
            .iter()
            .map(|p| CustomProbeConfig {
                name: p.name.clone(),
                command: p.command.clone(),
                timeout_secs: p.timeout_secs,
            })
            .collect()
    });
    HealthCheckConfig {
        enable: h.enable,
        timeout_secs: h.timeout_secs,
        systemd,
        tcp_ports: h.tcp_ports.clone(),
        http_probes,
        custom_probes,
    }
}

#[async_trait]
impl ConfigStorePort for TomlConfigStore {
    async fn resolve(&self, host: &HostEntity) -> Result<SshProfile, NodError> {
        let merged = self.merged_for(&host.name).ssh;
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
        if let Some(proxy) = merged.proxy_command {
            profile = profile.with_proxy_command(proxy);
        }
        if let Some(sudo) = merged.sudo {
            profile = profile.with_sudo(sudo);
        }
        if let Some(timeout) = merged.timeout_secs {
            profile = profile.with_timeout_secs(timeout);
        }
        if let Some(connect) = merged.connect_timeout_secs {
            profile = profile.with_connect_timeout_secs(connect);
        }
        if let Some(args) = merged.extra_ssh_args {
            for arg in args {
                profile = profile.with_extra_ssh_arg(arg);
            }
        }
        if let Some(insecure) = merged.allow_insecure {
            profile = profile.with_allow_insecure(insecure);
        }
        Ok(profile)
    }

    async fn host_overrides(&self, name: &str) -> Result<HostOverrides, NodError> {
        let merged = self.merged_for(name).ssh;
        let host = self.toml.hosts.get(name);
        Ok(HostOverrides {
            ssh: SshConnectionOverrides {
                user: merged.user,
                port: merged.port,
                identity_file: merged.identity_file,
                proxy_jump: merged.proxy_jump,
                proxy_command: merged.proxy_command,
                sudo: merged.sudo,
                timeout_secs: merged.timeout_secs,
                connect_timeout_secs: merged.connect_timeout_secs,
                extra_ssh_args: merged.extra_ssh_args,
                allow_insecure: merged.allow_insecure,
            },
            target_host: host.and_then(|h| h.target_host.clone()),
            description: host.and_then(|h| h.description.clone()),
            role: host.and_then(|h| h.role.clone()),
            tags: host.and_then(|h| h.tags.clone()),
            build: host.and_then(|h| h.build.as_ref()).map(to_build_config),
            rollout: host.and_then(|h| h.rollout.as_ref()).map(to_rollout_config),
            health_checks: host
                .and_then(|h| h.health_checks.as_ref())
                .map(to_health_config),
            hooks: host.and_then(|h| h.hooks.as_ref()).map(to_hooks_config),
        })
    }

    async fn fleet_defaults(&self) -> Result<FleetDefaults, NodError> {
        let merged = self.base_merged().ssh;
        // CLI overrides are per-run, not fleet defaults.
        Ok(FleetDefaults {
            ssh: SshConnectionOverrides {
                user: merged.user,
                port: merged.port,
                identity_file: merged.identity_file,
                proxy_jump: merged.proxy_jump,
                proxy_command: merged.proxy_command,
                sudo: merged.sudo,
                timeout_secs: merged.timeout_secs,
                connect_timeout_secs: merged.connect_timeout_secs,
                extra_ssh_args: merged.extra_ssh_args,
                allow_insecure: merged.allow_insecure,
            },
            description: None,
            build: None,
            rollout: None,
            health_checks: None,
            hooks: None,
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
        write_toml(
            root,
            "[defaults]\nuser = \"deploy\"\n[fleet]\nuser = \"fleet\"\n",
        );
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
        let cli = CliOverrides {
            user: Some("cli-user".to_string()),
            port: None,
            identity_file: None,
        };
        let s = store(root, cli);
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "cli-user");
    }

    #[tokio::test]
    async fn cli_port_overrides_toml_section() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[hosts.atlas]\nport = 2222\n");
        let cli = CliOverrides {
            user: None,
            port: Some(2200),
            identity_file: None,
        };
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
        assert_eq!(
            atlas.identity_file(),
            Some(PathBuf::from("/var/lib/nod/id")).as_ref()
        );
        assert_eq!(atlas.proxy_jump(), Some("bastion.example.org"));
        assert!(atlas.sudo());

        // A host without its own section inherits the fleet identity file.
        let orbit = s.resolve(&host("orbit")).await.unwrap();
        assert_eq!(
            orbit.identity_file(),
            Some(PathBuf::from("/var/lib/nod/id")).as_ref()
        );
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
        assert_eq!(overrides.ssh.user, Some("philipp".to_string()));
        assert_eq!(overrides.role, Some("server".to_string()));
        assert_eq!(
            overrides.tags,
            Some(vec!["prod".to_string(), "web".to_string()])
        );
    }

    #[tokio::test]
    async fn fleet_defaults_omit_cli_overrides() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[fleet]\nuser = \"fleet\"\nport = 2222\n");
        let cli = CliOverrides {
            user: Some("cli".to_string()),
            port: Some(99),
            identity_file: None,
        };
        let s = store(root, cli);
        let defaults = s.fleet_defaults().await.unwrap();
        assert_eq!(defaults.ssh.user, Some("fleet".to_string()));
        assert_eq!(defaults.ssh.port, Some(2222));
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
        let err = TomlConfigStore::new(root, CliOverrides::default())
            .err()
            .unwrap();
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
