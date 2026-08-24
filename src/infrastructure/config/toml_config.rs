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
//!
//! The `[defaults]` section also carries the optional default flake root,
//! `[defaults].flake`. Because the file itself is discovered bottom-up from
//! the flake root, a flake root supplied by the file needs a bootstrap pass:
//! [`effective_flake`] searches upward from the invocation cwd first, honours
//! `[defaults].flake` (relative values resolve against the file's own
//! directory), and only then this store is built from the resolved root. The
//! flake cascade is: explicit CLI `--flake` > `[defaults].flake` > `.`.

use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::domain::config::{
    BuildConfig, CliOverrides, CustomProbeConfig, FleetDefaults, HealthCheckConfig, HooksConfig,
    HostOverrides, HttpProbeConfig, RolloutConfig, SshConnectionOverrides, SshProfileConfig,
    SystemdHealthConfig,
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

/// The `[defaults]` section: the shared SSH settings (same keys as `[fleet]`)
/// plus the optional default flake root. `flake` is an ADR-004 tier-2 value
/// applied by [`effective_flake`] when no explicit `--flake` is given.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlDefaults {
    /// Optional default flake root. Relative values are resolved against the
    /// directory containing the `.nod.toml` file by [`effective_flake`].
    pub flake: Option<PathBuf>,
    #[serde(flatten)]
    pub ssh: SshOverrides,
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

/// Parsed shape of `.nod.toml`. `[defaults]` is [`TomlDefaults`]: the shared
/// SSH settings plus the optional default flake root.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TomlConfig {
    #[serde(default)]
    pub defaults: Option<TomlDefaults>,
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

    /// Returns the raw `[defaults].flake` value from the parsed config file,
    /// if any. Relative values are relative to the file's directory; call
    /// [`effective_flake`] to resolve the effective root for a run.
    pub fn default_flake(&self) -> Option<&Path> {
        self.toml.defaults.as_ref().and_then(|d| d.flake.as_deref())
    }

    /// Finds the nearest config file at or above `start`. `start` need not be
    /// the flake root: [`effective_flake`] searches from the invocation cwd so
    /// a `[defaults].flake` in the file can bootstrap the root itself.
    ///
    /// A relative `start` is resolved against the current working directory
    /// first (see `absolute_start`): Rust's `Path::parent` caps the ancestor
    /// walk of a bare `.` immediately at `Some("")->None`, so a search started
    /// from the invocation `cwd` would otherwise never climb above it.
    fn discover(start: &Path) -> Option<PathBuf> {
        let start = Self::absolute_start(start);
        let mut dir: Option<&Path> = Some(&start);
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

    /// Resolves a possibly-relative discovery start against the current
    /// working directory so the ancestor walk can climb above a bare `'.'`
    /// (`Path::new(".").parent()` is `Some("")`, whose own parent is `None`;
    /// the walk would otherwise stop after checking the cwd itself).
    /// Absolute starts pass through unchanged. Falls back to the unmodified
    /// start if the cwd cannot be determined.
    fn absolute_start(start: &Path) -> PathBuf {
        if start.is_absolute() {
            return start.to_path_buf();
        }
        std::env::current_dir()
            .map(|cwd| cwd.join(start))
            .unwrap_or_else(|_| start.to_path_buf())
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
            merged.overlay(&d.ssh);
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

/// Resolves the effective flake root for one run (ADR-004 cascade):
///
/// 1. an explicit CLI `--flake` value (anything but the documented default
///    `"."`) wins;
/// 2. else `[defaults].flake` of the nearest `.nod.toml` discovered walking
///    up from `search_start` (the invocation cwd), resolved relative to the
///    directory containing that file;
/// 3. else `"."` — the working directory.
///
/// The `"."` marker doubles as "no explicit choice" because the clap string
/// flags default to it and it is the historical hard-coded fallback; an
/// explicit `--flake .` is therefore indistinguishable from an absent flag and
/// does not override the toml tier. The config file is searched from the
/// invocation cwd, not from the flake root — learning the root from the file
/// is the whole point (allows `nod <cmd>` outside the configured flake dir).
pub fn effective_flake(cli_flake: &Path, search_start: &Path) -> Result<PathBuf, NodError> {
    if cli_flake != Path::new(".") {
        return Ok(cli_flake.to_path_buf());
    }
    if let Some(config) = TomlConfigStore::discover(search_start) {
        let toml = TomlConfigStore::parse(&config)?;
        if let Some(flake) = toml.defaults.as_ref().and_then(|d| d.flake.as_ref()) {
            return Ok(resolve_default_flake(&config, flake));
        }
    }
    Ok(PathBuf::from("."))
}

/// Resolves a raw `[defaults].flake` value against the directory containing
/// its config file. A config file anchored directly at a filesystem root has
/// no parent directory; the raw value is then returned unchanged.
fn resolve_default_flake(config: &Path, flake: &Path) -> PathBuf {
    match config.parent() {
        Some(dir) => dir.join(flake),
        None => flake.to_path_buf(),
    }
}

/// Maps the flake tier-3 SSH config surface (ADR-004, `config.nod.ssh`)
/// onto the store's internal override type so it can participate in the
/// standard `Merged::overlay` cascade as the lowest SSH tier.
fn to_ssh_overrides(c: &SshProfileConfig) -> SshOverrides {
    SshOverrides {
        user: c.user.clone(),
        port: c.port,
        identity_file: c.identity_file.clone(),
        proxy_jump: c.proxy_jump.clone(),
        proxy_command: c.proxy_command.clone(),
        sudo: c.sudo,
        timeout_secs: c.timeout_secs,
        connect_timeout_secs: c.connect_timeout_secs,
        extra_ssh_args: c.extra_ssh_args.clone(),
        allow_insecure: c.allow_insecure,
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
        // Tier 3 first (lowest SSH tier): flake metadata from `config.nod.ssh`
        // (ADR-004) materialized on the entity by `NixCliEvaluator`. Then tiers
        // 2+1 (`.nod.toml`, then CLI) are overlaid so they win on collision.
        let mut merged = Merged::default();
        merged.overlay(&to_ssh_overrides(&host.nod_config.ssh));
        merged.overlay(&self.merged_for(&host.name).ssh);
        let mut profile = SshProfile::for_host(host);
        if let Some(user) = merged.ssh.user {
            profile = profile.with_user(user);
        }
        if let Some(port) = merged.ssh.port {
            profile = profile.with_port(port);
        }
        if let Some(identity) = merged.ssh.identity_file {
            profile = profile.with_identity_file(identity);
        }
        if let Some(proxy) = merged.ssh.proxy_jump {
            profile = profile.with_proxy_jump(proxy);
        }
        if let Some(proxy) = merged.ssh.proxy_command {
            profile = profile.with_proxy_command(proxy);
        }
        if let Some(sudo) = merged.ssh.sudo {
            profile = profile.with_sudo(sudo);
        }
        if let Some(timeout) = merged.ssh.timeout_secs {
            profile = profile.with_timeout_secs(timeout);
        }
        if let Some(connect) = merged.ssh.connect_timeout_secs {
            profile = profile.with_connect_timeout_secs(connect);
        }
        if let Some(args) = merged.ssh.extra_ssh_args {
            for arg in args {
                profile = profile.with_extra_ssh_arg(arg);
            }
        }
        if let Some(insecure) = merged.ssh.allow_insecure {
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
            // `[defaults].flake` (tier 2); `[fleet]` has no flake key.
            flake: self.toml.defaults.as_ref().and_then(|d| d.flake.clone()),
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

    /// Builds a host carrying ADR-004 tier-3 flake SSH metadata, the value
    /// the `NixCliEvaluator` materializes from `config.nod.ssh`.
    fn host_with_flake_ssh(name: &str) -> HostEntity {
        let mut h = host(name);
        h.nod_config.ssh.identity_file = Some(PathBuf::from("/flake/id_rsa"));
        h.nod_config.ssh.proxy_jump = Some("jump.example".to_string());
        h.nod_config.ssh.sudo = Some(true);
        h
    }

    #[tokio::test]
    async fn flake_ssh_identity_resolves_without_lower_override() {
        // AC1: tier-3 flake metadata is honored when no `.nod.toml` / CLI
        // override supplies the value.
        let dir = tempdir().unwrap();
        let s = store(dir.path(), CliOverrides::default());
        let profile = s.resolve(&host_with_flake_ssh("atlas")).await.unwrap();
        assert_eq!(
            profile.identity_file(),
            Some(&PathBuf::from("/flake/id_rsa"))
        );
        assert_eq!(profile.proxy_jump(), Some("jump.example"));
        assert!(profile.sudo());
    }

    #[tokio::test]
    async fn toml_overrides_flake_ssh_identity() {
        // AC2: `.nod.toml` (tier 2) beats flake metadata (tier 3).
        let dir = tempdir().unwrap();
        write_toml(
            dir.path(),
            "[hosts.atlas.ssh]\nidentity_file = \"/toml/id_rsa\"\n",
        );
        let s = store(dir.path(), CliOverrides::default());
        let profile = s.resolve(&host_with_flake_ssh("atlas")).await.unwrap();
        assert_eq!(
            profile.identity_file(),
            Some(&PathBuf::from("/toml/id_rsa"))
        );
        // Non-colliding field still comes from flake.
        assert_eq!(profile.proxy_jump(), Some("jump.example"));
    }

    #[tokio::test]
    async fn cli_overrides_flake_and_toml_ssh_identity() {
        // AC3: CLI (tier 1) beats `.nod.toml` (tier 2) and flake (tier 3).
        let dir = tempdir().unwrap();
        write_toml(
            dir.path(),
            "[hosts.atlas.ssh]\nidentity_file = \"/toml/id_rsa\"\n",
        );
        let cli = CliOverrides {
            user: None,
            port: None,
            identity_file: Some(PathBuf::from("/cli/id_rsa")),
        };
        let s = store(dir.path(), cli);
        let profile = s.resolve(&host_with_flake_ssh("atlas")).await.unwrap();
        assert_eq!(profile.identity_file(), Some(&PathBuf::from("/cli/id_rsa")));
        assert_eq!(profile.proxy_jump(), Some("jump.example"));
    }

    #[tokio::test]
    async fn absent_flake_ssh_is_a_noop() {
        // AC4: default NodConfig (all fields None) resolves exactly as before.
        let dir = tempdir().unwrap();
        let s = store(dir.path(), CliOverrides::default());
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "root");
        assert_eq!(profile.port(), 22);
        assert_eq!(profile.identity_file(), None);
        assert_eq!(profile.proxy_jump(), None);
        assert!(!profile.sudo());
    }

    #[tokio::test]
    async fn defaults_flake_parses_and_exposes_accessor_and_fleet_defaults() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(
            root,
            "[defaults]\nflake = \"/etc/nixos\"\nuser = \"deploy\"\n",
        );
        let s = store(root, CliOverrides::default());

        // Accessor exposes the raw `[defaults].flake` value.
        assert_eq!(s.default_flake(), Some(Path::new("/etc/nixos")));

        // The domain defaults record carries it too (ADR-004 tier 2).
        let defaults = s.fleet_defaults().await.unwrap();
        assert_eq!(defaults.flake, Some(PathBuf::from("/etc/nixos")));

        // Non-flake `[defaults]` keys still parse as shared SSH settings.
        let profile = s.resolve(&host("atlas")).await.unwrap();
        assert_eq!(profile.user(), "deploy");
    }

    #[test]
    fn effective_flake_prefers_cli_then_toml_then_default() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[defaults]\nflake = \"/srv/nixos/flakes\"\n");

        // An explicit CLI value wins, even when relative.
        assert_eq!(
            effective_flake(Path::new("nix/flakes"), root).unwrap(),
            PathBuf::from("nix/flakes")
        );

        // The `.` marker (no explicit choice) falls through to the toml tier.
        assert_eq!(
            effective_flake(Path::new("."), root).unwrap(),
            PathBuf::from("/srv/nixos/flakes")
        );

        // No config file anywhere and no explicit flag means `.`.
        let empty = tempdir().unwrap();
        assert_eq!(
            effective_flake(Path::new("."), empty.path()).unwrap(),
            PathBuf::from(".")
        );
    }

    #[test]
    fn effective_flakes_relative_default_flake_resolves_to_config_location() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[defaults]\nflake = \"machines/web-01\"\n");

        let flake = effective_flake(Path::new("."), root).unwrap();
        assert_eq!(flake, root.join("machines/web-01"));

        // Discovery still walks up from a subdirectory of the config dir.
        let sub = root.join("deep").join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            effective_flake(Path::new("."), &sub).unwrap(),
            root.join("machines/web-01")
        );
    }

    #[test]
    fn effective_flake_ignores_toml_without_flake_key() {
        let dir = tempdir().unwrap();
        write_toml(dir.path(), "[defaults]\nuser = \"deploy\"\n");
        assert_eq!(
            effective_flake(Path::new("."), dir.path()).unwrap(),
            PathBuf::from(".")
        );
    }

    #[test]
    fn effective_flake_surfaces_invalid_config() {
        let dir = tempdir().unwrap();
        write_toml(dir.path(), "this is not = [valid toml");
        let err = effective_flake(Path::new("."), dir.path()).unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[test]
    fn explicit_dot_flake_does_not_override_defaults_flake() {
        // Documented pin (review WARNING): the `'.'` marker doubles as "no
        // explicit choice" because clap defaults the string flags to it, so an
        // explicit `--flake .` is indistinguishable from an absent flag and
        // must NOT override the toml tier (no escape hatch, by design).
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_toml(root, "[defaults]\nflake = \"/srv/nixos\"\n");

        assert_eq!(
            effective_flake(Path::new("."), root).unwrap(),
            PathBuf::from("/srv/nixos")
        );
    }

    #[test]
    fn effective_flake_discovers_nod_toml_alt_filename() {
        // `nod.toml` (no leading dot) is the second name in CONFIG_FILE_NAMES
        // and must constrain the cascade exactly like `.nod.toml`.
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("nod.toml"),
            "[defaults]\nflake = \"flakes/web-01\"\n",
        )
        .unwrap();

        assert_eq!(
            effective_flake(Path::new("."), root).unwrap(),
            root.join("flakes/web-01")
        );
    }

    #[test]
    fn absolute_start_resolves_relative_starts_against_cwd() {
        let cwd = std::env::current_dir().unwrap();

        // The production value: `Path::new(".")` normalizes to the absolute
        // cwd so the ancestor walk can climb above it.
        assert_eq!(
            TomlConfigStore::absolute_start(Path::new(".")),
            cwd.join(".")
        );
        assert!(TomlConfigStore::absolute_start(Path::new(".")).is_absolute());
        assert!(TomlConfigStore::absolute_start(Path::new("")).is_absolute());

        // Multi-component relative starts keep their structure.
        assert_eq!(
            TomlConfigStore::absolute_start(Path::new("deep/nested")),
            cwd.join("deep/nested")
        );

        // Absolute starts pass through unchanged.
        assert_eq!(
            TomlConfigStore::absolute_start(Path::new("/etc/nixos")),
            PathBuf::from("/etc/nixos")
        );
    }

    #[test]
    fn root_anchored_config_resolves_default_flake_without_parent() {
        // The discover walk can only return a config directly anchored at `/`
        // when run as root, so the parent-less branch is exercised directly on
        // the resolver: the raw value is returned unchanged.
        assert_eq!(
            resolve_default_flake(Path::new("/"), Path::new("flakes/vm")),
            PathBuf::from("flakes/vm")
        );

        // A normal file still resolves relative defaults against its directory.
        assert_eq!(
            resolve_default_flake(Path::new("/etc/nixos/.nod.toml"), Path::new("flakes/vm")),
            PathBuf::from("/etc/nixos/flakes/vm")
        );
    }
}
