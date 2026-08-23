//! Typed configuration value objects for the merged four-tier hierarchy
//! (ADR-004). These are immutable, dependency-free domain values; tiers are
//! resolved once by the config store and exposed in merged form here.
//!
//! Precedence (fixed per option, higher beats lower):
//!   1. CLI overrides  (`CliOverrides`)
//!   2. Local `.nod.toml`  (`[hosts.<name>]` > `[fleet]` > `[defaults]`)
//!   3. Flake metadata (materialized on the `HostEntity`)
//!   4. Compiled-in defaults (`root` / `22`, also on the `HostEntity`)
//!
//! The granular option structs (`SshProfileConfig`, `BuildConfig`,
//! `RolloutConfig`, `HealthCheckConfig`, `HooksConfig`, `NodConfig`) mirror
//! the `options.nod` surface of the NixOS module exactly, so the same merged
//! values round-trip through `config.nod` (Nix style, camelCase) and
//! `.nod.toml` (snake_case / kebab-case). `rename_all = "camelCase"` reads the
//! Nix spelling; per-field `#[serde(alias = "...")]` accepts the snake_case
//! TOML spelling onto the same field.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Highest-tier, per-run SSH overrides supplied by CLI flags. Applied on top
/// of every resolved profile after the `.nod.toml` and flake tiers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliOverrides {
    /// CLI `--user` override, empty when not supplied.
    pub user: Option<String>,
    /// CLI `--port` override, empty when not supplied.
    pub port: Option<u16>,
    /// CLI `--identity-file` override, empty when not supplied.
    pub identity_file: Option<PathBuf>,
}

/// An immutable SSH connection descriptor (ADR-001 value object) surfaced as
/// a granular config value. Mirrors `options.nod.ssh`; every field is optional
/// so an absent TOML/JSON section leaves the tier unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshProfileConfig {
    pub user: Option<String>,
    pub port: Option<u16>,
    #[serde(alias = "identity_file")]
    pub identity_file: Option<PathBuf>,
    #[serde(alias = "proxy_jump")]
    pub proxy_jump: Option<String>,
    #[serde(alias = "proxy_command")]
    pub proxy_command: Option<String>,
    pub sudo: Option<bool>,
    #[serde(alias = "timeout_secs")]
    pub timeout_secs: Option<u32>,
    #[serde(alias = "connect_timeout_secs")]
    pub connect_timeout_secs: Option<u32>,
    #[serde(alias = "extra_ssh_args")]
    pub extra_ssh_args: Option<Vec<String>>,
    #[serde(alias = "allow_insecure")]
    pub allow_insecure: Option<bool>,
}

/// Build customisation options (1:1 with `options.nod.build`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildConfig {
    #[serde(alias = "build_host")]
    pub build_host: Option<String>,
    pub substituters: Option<Vec<String>>,
    #[serde(alias = "trusted_public_keys")]
    pub trusted_public_keys: Option<Vec<String>>,
    #[serde(alias = "eval_flags")]
    pub eval_flags: Option<Vec<String>>,
    #[serde(alias = "show_trace")]
    pub show_trace: Option<bool>,
    pub impure: Option<bool>,
}

/// Rollout / activation options (1:1 with `options.nod.rollout`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutConfig {
    pub priority: Option<u32>,
    pub action: Option<String>,
    #[serde(alias = "auto_rollback")]
    pub auto_rollback: Option<bool>,
    pub reboot: Option<bool>,
    #[serde(alias = "magic_rollback")]
    pub magic_rollback: Option<bool>,
    #[serde(alias = "magic_rollback_timeout_secs")]
    pub magic_rollback_timeout_secs: Option<u32>,
}

/// systemd unit verification sub-settings (1:1 with `options.nod.healthChecks.systemd`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemdHealthConfig {
    #[serde(alias = "check_running")]
    pub check_running: Option<bool>,
    #[serde(alias = "check_failed_units")]
    pub check_failed_units: Option<bool>,
    #[serde(alias = "required_units")]
    pub required_units: Option<Vec<String>>,
}

/// One HTTP probe (1:1 with a `options.nod.healthChecks.httpProbes` entry).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpProbeConfig {
    pub url: Option<String>,
    #[serde(alias = "expected_status")]
    pub expected_status: Option<u16>,
    #[serde(alias = "timeout_secs")]
    pub timeout_secs: Option<u32>,
}

/// One custom probe (1:1 with a `options.nod.healthChecks.customProbes` entry).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProbeConfig {
    pub name: Option<String>,
    pub command: Option<String>,
    #[serde(alias = "timeout_secs")]
    pub timeout_secs: Option<u32>,
}

/// Health-verification options (1:1 with `options.nod.healthChecks`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckConfig {
    pub enable: Option<bool>,
    #[serde(alias = "timeout_secs")]
    pub timeout_secs: Option<u32>,
    #[serde(alias = "systemd")]
    pub systemd: SystemdHealthConfig,
    #[serde(alias = "tcp_ports")]
    pub tcp_ports: Option<Vec<u16>>,
    #[serde(alias = "http_probes")]
    pub http_probes: Option<Vec<HttpProbeConfig>>,
    #[serde(alias = "custom_probes")]
    pub custom_probes: Option<Vec<CustomProbeConfig>>,
}

/// Switch lifecycle hooks (1:1 with `options.nod.hooks`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HooksConfig {
    #[serde(alias = "pre_switch_hook")]
    pub pre_switch_hook: Option<String>,
    #[serde(alias = "post_switch_hook")]
    pub post_switch_hook: Option<String>,
}

/// The full flake `config.nod` surface (ADR-004 tier 3). Deserializes from
/// Nix-evaluated JSON (camelCase) and `.nod.toml` (snake_case via aliases).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodConfig {
    pub enable: Option<bool>,
    #[serde(alias = "target_host")]
    pub target_host: Option<String>,
    pub user: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub ssh: SshProfileConfig,
    pub build: BuildConfig,
    pub rollout: RolloutConfig,
    #[serde(alias = "health_checks")]
    pub health_checks: HealthCheckConfig,
    pub hooks: HooksConfig,
}

/// Merged per-host overrides exposed by the config store: TOML `[hosts.<name>]`
/// values over `[fleet]` over `[defaults]`. `target_host`, `role`, `tags` and
/// `description` are host-section-specific; the granular group overrides
/// (build/rollout/health/hooks) are optional and absent when not set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOverrides {
    pub target_host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub sudo: Option<bool>,
    pub timeout_secs: Option<u32>,
    pub connect_timeout_secs: Option<u32>,
    pub extra_ssh_args: Option<Vec<String>>,
    pub allow_insecure: Option<bool>,
    pub description: Option<String>,
    pub role: Option<String>,
    pub tags: Option<Vec<String>>,
    pub build: Option<BuildConfig>,
    pub rollout: Option<RolloutConfig>,
    pub health_checks: Option<HealthCheckConfig>,
    pub hooks: Option<HooksConfig>,
}

/// Fleet-wide merged defaults: TOML `[fleet]`/`[defaults]` values (persistent
/// policy) with no CLI tier. Ships the connection defaults that every host
/// inherits when it has no specific override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetDefaults {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub sudo: Option<bool>,
    pub timeout_secs: Option<u32>,
    pub connect_timeout_secs: Option<u32>,
    pub extra_ssh_args: Option<Vec<String>>,
    pub allow_insecure: Option<bool>,
    pub description: Option<String>,
    pub build: Option<BuildConfig>,
    pub rollout: Option<RolloutConfig>,
    pub health_checks: Option<HealthCheckConfig>,
    pub hooks: Option<HooksConfig>,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostEntity;

    /// A complete Nix `config.nod` JSON payload with every granular option
    /// populated, using the camelCase spelling the `NixCliEvaluator` emits
    /// from the module surface (ADR-004 tier 3). This is exactly the object a
    /// flake machine exposes through `options.nod`.
    const FULL_NOD_PAYLOAD: &str = r#"
    {
      "enable": true,
      "targetHost": "10.0.0.8",
      "user": "philipp",
      "role": "server",
      "tags": ["prod", "edge"],
      "description": "edge gateway",
      "ssh": {
        "user": "philipp",
        "port": 2222,
        "identityFile": "/etc/ssh/edge_key",
        "proxyJump": "bastion.example.org",
        "proxyCommand": "ssh -W %d:%p bastion.example.org",
        "sudo": true,
        "timeoutSecs": 45,
        "connectTimeoutSecs": 12,
        "extraSshArgs": ["-Z"],
        "allowInsecure": true
      },
      "build": {
        "buildHost": "buildy",
        "substituters": ["https://cache.example.nix"],
        "trustedPublicKeys": ["https://cache.example/nix-cache.pub"],
        "evalFlags": ["--impure"],
        "showTrace": true,
        "impure": true
      },
      "rollout": {
        "priority": 10,
        "action": "switch",
        "autoRollback": false,
        "reboot": true,
        "magicRollback": false,
        "magicRollbackTimeoutSecs": 120
      },
      "healthChecks": {
        "enable": true,
        "timeoutSecs": 60,
        "systemd": {
          "checkRunning": true,
          "checkFailedUnits": false,
          "requiredUnits": ["foo.service", "bar.service"]
        },
        "tcpPorts": [443, 8443],
        "httpProbes": [
          {
            "url": "https://edge.example.org/health",
            "expectedStatus": 200,
            "timeoutSecs": 15
          }
        ],
        "customProbes": [
          {
            "name": "disk-space",
            "command": "df -h /",
            "timeoutSecs": 20
          }
        ]
      },
      "hooks": {
        "preSwitchHook": "echo pre",
        "postSwitchHook": "echo post"
      }
    }"#;

    #[test]
    fn complete_config_nod_payload_deserializes_with_all_fields() {
        let nod = serde_json::from_str::<NodConfig>(FULL_NOD_PAYLOAD).unwrap();

        // Top-level module surface.
        assert_eq!(nod.enable, Some(true));
        assert_eq!(nod.target_host, Some("10.0.0.8".to_string()));
        assert_eq!(nod.user, Some("philipp".to_string()));
        assert_eq!(nod.role, Some("server".to_string()));
        assert_eq!(nod.tags, vec!["prod".to_string(), "edge".to_string()]);
        assert_eq!(nod.description, Some("edge gateway".to_string()));

        // SSH group.
        assert_eq!(nod.ssh.user, Some("philipp".to_string()));
        assert_eq!(nod.ssh.port, Some(2222));
        assert_eq!(
            nod.ssh.identity_file,
            Some(PathBuf::from("/etc/ssh/edge_key"))
        );
        assert_eq!(nod.ssh.proxy_jump, Some("bastion.example.org".to_string()));
        assert_eq!(
            nod.ssh.proxy_command,
            Some("ssh -W %d:%p bastion.example.org".to_string())
        );
        assert_eq!(nod.ssh.sudo, Some(true));
        assert_eq!(nod.ssh.timeout_secs, Some(45));
        assert_eq!(nod.ssh.connect_timeout_secs, Some(12));
        assert_eq!(nod.ssh.extra_ssh_args, Some(vec!["-Z".to_string()]));
        assert_eq!(nod.ssh.allow_insecure, Some(true));

        // Build group.
        assert_eq!(nod.build.build_host, Some("buildy".to_string()));
        assert_eq!(
            nod.build.substituters,
            Some(vec!["https://cache.example.nix".to_string()])
        );
        assert_eq!(
            nod.build.trusted_public_keys,
            Some(vec!["https://cache.example/nix-cache.pub".to_string()])
        );
        assert_eq!(nod.build.eval_flags, Some(vec!["--impure".to_string()]));
        assert_eq!(nod.build.show_trace, Some(true));
        assert_eq!(nod.build.impure, Some(true));

        // Rollout group.
        assert_eq!(nod.rollout.priority, Some(10));
        assert_eq!(nod.rollout.action, Some("switch".to_string()));
        assert_eq!(nod.rollout.auto_rollback, Some(false));
        assert_eq!(nod.rollout.reboot, Some(true));
        assert_eq!(nod.rollout.magic_rollback, Some(false));
        assert_eq!(nod.rollout.magic_rollback_timeout_secs, Some(120));

        // Health-check group.
        assert_eq!(nod.health_checks.enable, Some(true));
        assert_eq!(nod.health_checks.timeout_secs, Some(60));
        assert_eq!(nod.health_checks.systemd.check_running, Some(true));
        assert_eq!(nod.health_checks.systemd.check_failed_units, Some(false));
        assert_eq!(
            nod.health_checks.systemd.required_units,
            Some(vec!["foo.service".to_string(), "bar.service".to_string()])
        );
        assert_eq!(nod.health_checks.tcp_ports, Some(vec![443, 8443]));

        let probes = nod.health_checks.http_probes.unwrap();
        assert_eq!(probes.len(), 1);
        assert_eq!(
            probes[0].url,
            Some("https://edge.example.org/health".to_string())
        );
        assert_eq!(probes[0].expected_status, Some(200));
        assert_eq!(probes[0].timeout_secs, Some(15));

        let customs = nod.health_checks.custom_probes.unwrap();
        assert_eq!(customs.len(), 1);
        assert_eq!(customs[0].name, Some("disk-space".to_string()));
        assert_eq!(customs[0].command, Some("df -h /".to_string()));
        assert_eq!(customs[0].timeout_secs, Some(20));

        // Hooks group.
        assert_eq!(nod.hooks.pre_switch_hook, Some("echo pre".to_string()));
        assert_eq!(nod.hooks.post_switch_hook, Some("echo post".to_string()));
    }

    #[test]
    fn complete_nod_payload_materializes_onto_the_host_entity() {
        let nod = serde_json::from_str::<NodConfig>(FULL_NOD_PAYLOAD).unwrap();
        let mut entity = HostEntity::new("edge", "10.0.0.8", false);
        entity.nod_config = nod;

        assert_eq!(entity.nod_config.enable, Some(true));
        assert_eq!(entity.nod_config.target_host, Some("10.0.0.8".to_string()));
        assert_eq!(entity.nod_config.ssh.port, Some(2222));
        assert_eq!(entity.nod_config.rollout.action, Some("switch".to_string()));
        assert_eq!(
            entity.nod_config.health_checks.tcp_ports,
            Some(vec![443, 8443])
        );
        assert!(entity.nod_config.hooks.pre_switch_hook.is_some());
    }
}
