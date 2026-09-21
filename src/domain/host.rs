//! Domain entities and value objects.
//!
//! `HostEntity` is the long-lived, stateful entity describing a NixOS
//! configurable target. `HostRole`, `SshProfile` and `TargetHost` are
//! immutable value objects derived from it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::config::NodConfig;

/// The functional role of a host within a fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostRole {
    Desktop,
    Notebook,
    Server,
    Router,
    Embedded,
    Cloud,
    Unknown(String),
}

impl HostRole {
    /// Parses a flake `deployment.role` string into a typed `HostRole`.
    /// Unknown role strings map to the `Unknown(String)` variant.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "desktop" => HostRole::Desktop,
            "notebook" => HostRole::Notebook,
            "server" => HostRole::Server,
            "router" => HostRole::Router,
            "embedded" => HostRole::Embedded,
            "cloud" => HostRole::Cloud,
            _ => HostRole::Unknown(s.to_string()),
        }
    }

    /// Stable string form for persistence and display.
    pub fn to_str(&self) -> String {
        match self {
            HostRole::Desktop => String::from("desktop"),
            HostRole::Notebook => String::from("notebook"),
            HostRole::Server => String::from("server"),
            HostRole::Router => String::from("router"),
            HostRole::Embedded => String::from("embedded"),
            HostRole::Cloud => String::from("cloud"),
            HostRole::Unknown(s) => s.clone(),
        }
    }
}

impl std::fmt::Display for HostRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

/// The deployment execution modality of a target host (ADR-026).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Standard NixOS host with switch-to-configuration.
    #[default]
    Nixos,
    /// Agentless target executing an activation package locally against APIs/devices.
    Agentless,
    /// Remote script target copied and executed on the target host.
    RemoteScript,
}

impl TargetKind {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "agentless" | "api" | "local" => TargetKind::Agentless,
            "remote" | "remote_script" | "remotescript" => TargetKind::RemoteScript,
            _ => TargetKind::Nixos,
        }
    }

    /// Whether a target of this kind is activated *on the machine running nod*.
    ///
    /// This is a property of the modality and needs no separate field: an agentless
    /// target reconciles through a device API from here, while a NixOS or remote-script
    /// target is activated where it lives. It answers "does the activation run here",
    /// which is a different question from "is this machine the target"
    /// ([`HostEntity::is_self`]) - conflating the two is what once made `drift` compare a
    /// device's reconciler package against this machine's running system.
    pub fn activation_executes_locally(&self) -> bool {
        matches!(self, TargetKind::Agentless)
    }

    /// Whether a target of this kind has a shell nod can open over SSH.
    ///
    /// A device activated through an API has none: `ssh` and `exec` are not subjects it
    /// can satisfy, however reachable it is. Asking the modality keeps that answer in one
    /// place instead of in every command's condition.
    pub fn has_command_channel(&self) -> bool {
        matches!(self, TargetKind::Nixos | TargetKind::RemoteScript)
    }

    /// Whether a target of this kind has a running system whose generation can be compared.
    pub fn has_running_system(&self) -> bool {
        matches!(self, TargetKind::Nixos)
    }
}

/// A NixOS or generic configurable target host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntity {
    pub name: String,
    pub target_host: String,
    pub target_user: String,
    pub target_port: u16,
    pub role: HostRole,
    #[serde(default)]
    pub target_kind: TargetKind,
    pub is_self: bool,
    pub active_closure: Option<PathBuf>,
    /// The flake attribute this target's closure is built from, or `None` when the
    /// target declares none at all - an inventory-only target such as the router,
    /// the access point or a relay: nod tracks its reachability and nothing else
    /// (ADR-026).
    #[serde(default)]
    pub closure_attr: Option<String>,
    /// Operator-assignable tags for fleet filtering (`--tag`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Full `config.nod` surface deserialized from the flake (tier 3) and the
    /// `.nod.toml` overrides: ssh / build / rollout / health / hooks.
    #[serde(default)]
    pub nod_config: NodConfig,
}

impl HostEntity {
    /// Constructs a host with the compiled-in defaults (user `root`,
    /// port `22`, role `server`, no tags, no active closure).
    ///
    /// It declares the toplevel closure of a `nixosConfigurations` host, which is the
    /// same default `target_kind` carries: a host built by name is a NixOS host until
    /// something says otherwise. Discovery overrides both explicitly - the attribute
    /// with what the flake actually declares, the kind with what the target declares -
    /// so an inventory target never inherits this one.
    pub fn new(name: impl Into<String>, target_host: impl Into<String>, is_self: bool) -> Self {
        let name = name.into();
        Self {
            name: name.clone(),
            target_host: target_host.into(),
            target_user: "root".to_string(),
            target_port: 22,
            role: HostRole::Server,
            target_kind: TargetKind::Nixos,
            is_self,
            active_closure: None,
            closure_attr: Some(format!(
                "#nixosConfigurations.{name}.config.system.build.toplevel"
            )),
            tags: Vec::new(),
            nod_config: NodConfig::default(),
        }
    }

    /// Returns `true` when the host carries `tag` in its tag set.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Returns `true` when the host's role matches `role` (case-insensitive;
    /// unknown role strings compare by name).
    pub fn matches_role(&self, role: &str) -> bool {
        self.role.to_str().eq_ignore_ascii_case(role)
    }

    /// Returns the functional role of this host.
    pub fn role(&self) -> &HostRole {
        &self.role
    }

    /// True when this target declares something to build, compare and activate.
    ///
    /// This is the one definition of the question every lifecycle command has to
    /// answer (ADR-026): a target without a closure cannot drift, be rolled back or
    /// be switched. It is a property of the declaration, not of the activation
    /// modality - a `Nixos` and an `Agentless` target both have one, an
    /// inventory-only target has none - so callers ask *this*, never
    /// `target_kind`.
    pub fn has_closure(&self) -> bool {
        self.closure_attr.is_some()
    }

    /// Whether this target's activation executes on the machine running nod.
    ///
    /// True for the local machine itself - a NixOS host that *is* this machine is activated
    /// here - and for a target whose modality reconciles from here
    /// ([`TargetKind::activation_executes_locally`], a device configured through its API).
    ///
    /// It is deliberately *not* [`HostEntity::is_self`]: a device is not this machine and its
    /// activation still runs here. The deployment dispatch asks this; every question about
    /// reading a running system or opening a shell on the target asks `is_self`.
    pub fn activation_executes_locally(&self) -> bool {
        self.is_self || self.target_kind.activation_executes_locally()
    }

    /// Returns the SSH user to use when targeting this host.
    pub fn target_user(&self) -> &str {
        &self.target_user
    }

    /// Returns the SSH port to use when targeting this host.
    pub fn target_port(&self) -> u16 {
        self.target_port
    }

    /// Returns `true` when the host is the machine nod runs on.
    pub fn is_self(&self) -> bool {
        self.is_self
    }

    /// Derives the connection descriptor (SshProfile) for this host.
    ///
    /// This is a *primitive, non-resolved* profile (user/port/sudo only) and
    /// must not be mistaken for the effective profile. Authoritative
    /// resolution is `ConfigStorePort::resolve` / `AppContext::resolved_profile`
    /// (ADR-007); this convenience is for discovery-oriented contexts.
    pub fn ssh_profile(&self) -> SshProfile {
        SshProfile::for_host(self)
    }
}

/// An immutable SSH connection descriptor (ADR-001 value object).
///
/// Connection values are copied on every derivation; they are never mutated
/// in place. Configuration methods return a *new* profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshProfile {
    user: String,
    port: u16,
    identity_file: Option<PathBuf>,
    proxy_jump: Option<String>,
    proxy_command: Option<String>,
    timeout_secs: u32,
    connect_timeout_secs: u32,
    extra_ssh_args: Vec<String>,
    sudo: bool,
    allow_insecure: bool,
}

impl SshProfile {
    /// Builds a profile for a host using the host's resolved user and port
    /// (defaulting to `root`/`22` when not explicitly configured).
    pub fn for_host(host: &HostEntity) -> Self {
        Self {
            user: host.target_user.clone(),
            port: host.target_port,
            identity_file: None,
            proxy_jump: None,
            proxy_command: None,
            timeout_secs: 30,
            connect_timeout_secs: 10,
            extra_ssh_args: Vec::new(),
            sudo: host.is_self,
            allow_insecure: false,
        }
    }

    /// Builds an explicit profile.
    pub fn new(user: impl Into<String>, port: u16) -> Self {
        Self {
            user: user.into(),
            port,
            identity_file: None,
            proxy_jump: None,
            proxy_command: None,
            timeout_secs: 30,
            connect_timeout_secs: 10,
            extra_ssh_args: Vec::new(),
            sudo: true,
            allow_insecure: false,
        }
    }

    /// Returns the SSH user.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Returns the SSH port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the identity file, if configured.
    pub fn identity_file(&self) -> Option<&PathBuf> {
        self.identity_file.as_ref()
    }

    /// Returns the proxy-jump host, if configured.
    pub fn proxy_jump(&self) -> Option<&str> {
        self.proxy_jump.as_deref()
    }

    /// Returns an explicit proxy command (e.g. `ssh -W %d:%p bastion`).
    #[allow(dead_code)]
    pub fn proxy_command(&self) -> Option<&str> {
        self.proxy_command.as_deref()
    }

    /// Returns the connection timeout in seconds.
    pub fn timeout_secs(&self) -> u32 {
        self.timeout_secs
    }

    /// Returns the SSH connect timeout in seconds.
    #[allow(dead_code)]
    pub fn connect_timeout_secs(&self) -> u32 {
        self.connect_timeout_secs
    }

    /// Returns the additional SSH arguments.
    #[allow(dead_code)]
    pub fn extra_ssh_args(&self) -> &Vec<String> {
        &self.extra_ssh_args
    }

    /// Returns whether to allow insecure-but-needed SSH session options.
    #[allow(dead_code)]
    pub fn allow_insecure(&self) -> bool {
        self.allow_insecure
    }

    /// Returns whether the switch command should be escalated with sudo.
    pub fn sudo(&self) -> bool {
        self.sudo
    }

    /// Returns a copy with a different SSH user.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// Returns a copy with a different SSH port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Returns a copy with a different sudo escalation flag.
    pub fn with_sudo(mut self, sudo: bool) -> Self {
        self.sudo = sudo;
        self
    }

    /// Returns a copy with a different identity file.
    pub fn with_identity_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.identity_file = Some(path.into());
        self
    }

    /// Returns a copy with a different proxy-jump target.
    pub fn with_proxy_jump(mut self, proxy_jump: impl Into<String>) -> Self {
        self.proxy_jump = Some(proxy_jump.into());
        self
    }

    /// Returns a copy with a different proxy command.
    pub fn with_proxy_command(mut self, proxy_command: impl Into<String>) -> Self {
        self.proxy_command = Some(proxy_command.into());
        self
    }

    /// Returns a copy with a different connection timeout.
    pub fn with_timeout_secs(mut self, timeout_secs: u32) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Returns a copy with a different SSH connect timeout.
    pub fn with_connect_timeout_secs(mut self, connect_timeout_secs: u32) -> Self {
        self.connect_timeout_secs = connect_timeout_secs;
        self
    }

    /// Returns a copy with one more extra SSH argument appended.
    pub fn with_extra_ssh_arg(mut self, arg: impl Into<String>) -> Self {
        self.extra_ssh_args.push(arg.into());
        self
    }

    /// Returns a copy with the allow-insecure flag set.
    pub fn with_allow_insecure(mut self, allow_insecure: bool) -> Self {
        self.allow_insecure = allow_insecure;
        self
    }
}

/// A resolved deployment target: the host plus its immutable connection
/// descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetHost {
    pub name: String,
    pub profile: SshProfile,
    pub is_self: bool,
}

/// A builder fleet host: the CONNECT address of the single configured host on
/// which the toplevel closure is compiled remotely, plus the SSH profile used
/// to reach it (ADR-006 `nod build --builder`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderHost {
    /// Connect address of the builder host (`ssh://<user>@<target_host>`).
    pub target_host: String,
    /// Connection profile for the builder host (carries the credentials).
    pub profile: SshProfile,
}

impl TargetHost {
    /// Derives a target from a host entity.
    pub fn from_host(host: &HostEntity) -> Self {
        Self {
            name: host.name.clone(),
            profile: SshProfile::for_host(host),
            is_self: host.is_self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identity and modality are separate questions, and one boolean could never carry
    /// them: conflating them once made `drift` compare a device's reconciler package
    /// against this machine's running system (ADR-026 amendment).
    #[test]
    fn identity_and_modality_are_independent() {
        // A device reconciled from this machine: it is not this machine, its activation
        // runs here, it has no shell and no running system - but it does have a closure.
        let mut bridge = HostEntity::new("home-ap-01", "10.10.10.20", false);
        bridge.target_kind = TargetKind::Agentless;
        assert!(!bridge.is_self);
        assert!(bridge.target_kind.activation_executes_locally());
        assert!(!bridge.target_kind.has_command_channel());
        assert!(!bridge.target_kind.has_running_system());
        assert!(bridge.has_closure(), "its reconciler package is built");

        // A NixOS host somewhere else: nothing about it runs here.
        let remote = HostEntity::new("remote", "10.0.0.9", false);
        assert!(!remote.is_self);
        assert!(!remote.target_kind.activation_executes_locally());
        assert!(remote.target_kind.has_command_channel());
        assert!(remote.target_kind.has_running_system());
    }

    #[test]
    fn local_host_defaults() {
        let host = HostEntity::new("jello", "jello-machine", true);
        assert_eq!(host.name, "jello");
        assert!(host.is_self());
        assert_eq!(host.role, HostRole::Server);
        assert_eq!(host.target_user(), "root");
        assert_eq!(host.target_port(), 22);
        assert!(host.tags.is_empty());
        assert!(host.active_closure.is_none());
    }

    #[test]
    fn remote_host_has_no_active_closure() {
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        assert!(!host.is_self());
        assert!(host.active_closure.is_none());
        assert_eq!(host.role, HostRole::Server);
    }

    #[test]
    fn role_round_trips_through_serialization() {
        for role in [
            HostRole::Desktop,
            HostRole::Notebook,
            HostRole::Server,
            HostRole::Unknown("bastion".to_string()),
        ] {
            let json = serde_json::to_string(&role).unwrap();
            let back: HostRole = serde_json::from_str(&json).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn unknown_role_string_maps_to_unknown_variant() {
        assert_eq!(HostRole::parse("desktop"), HostRole::Desktop);
        assert_eq!(HostRole::parse("notebook"), HostRole::Notebook);
        assert_eq!(HostRole::parse("server"), HostRole::Server);
        assert_eq!(
            HostRole::parse("wobbly"),
            HostRole::Unknown("wobbly".to_string())
        );
        assert_eq!(HostRole::parse("wobbly").to_str(), "wobbly");
    }

    #[test]
    fn builder_host_round_trips_construction_and_serialization() {
        let profile = SshProfile::new("builder", 2222)
            .with_user("deploy")
            .with_port(2200);
        let builder = BuilderHost {
            target_host: "buildy".to_string(),
            profile,
        };
        let json = serde_json::to_string(&builder).unwrap();
        let back: BuilderHost = serde_json::from_str(&json).unwrap();
        assert_eq!(builder, back);
        assert_eq!(back.target_host, "buildy");
        assert_eq!(back.profile.user(), "deploy");
        assert_eq!(back.profile.port(), 2200);
    }

    #[test]
    fn builder_host_defaults_carry_root_user_and_port_22() {
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let builder = BuilderHost {
            target_host: "10.0.0.8".to_string(),
            profile: SshProfile::for_host(&host),
        };
        assert_eq!(builder.target_host, "10.0.0.8");
        assert_eq!(builder.profile.user(), "root");
        assert_eq!(builder.profile.port(), 22);
    }

    #[test]
    fn derived_profile_defaults_when_implicit() {
        let host = HostEntity::new("jello", "jello-machine", true);
        let profile = SshProfile::for_host(&host);
        assert_eq!(profile.user(), "root");
        assert_eq!(profile.port(), 22);
        assert_eq!(profile.timeout_secs(), 30);
        assert!(profile.sudo());
    }

    #[test]
    fn explicit_overrides_are_preserved() {
        let mut host = HostEntity::new("atlas", "10.0.0.8", false);
        host.target_user = String::from("philipp");
        host.target_port = 2222;
        let profile = SshProfile::for_host(&host);
        assert_eq!(profile.user(), "philipp");
        assert_eq!(profile.port(), 2222);
        assert!(!profile.sudo());
    }

    #[test]
    fn profile_values_are_copied_never_mutated_in_place() {
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let first = SshProfile::for_host(&host);
        let second = SshProfile::for_host(&host);
        assert_eq!(first, second);

        // Deriving a "changed" profile must not affect the original.
        let changed = SshProfile::for_host(&host).with_identity_file(PathBuf::from("/tmp/key"));
        assert_eq!(SshProfile::for_host(&host).identity_file(), None);
        assert_eq!(changed.user(), "root");
        assert!(changed.identity_file().is_some());
    }

    #[test]
    fn target_host_derives_name_profile_and_locality() {
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let target = TargetHost::from_host(&host);
        assert_eq!(target.name, "atlas");
        assert_eq!(target.profile.user(), "root");
        assert_eq!(target.profile.port(), 22);
        assert!(!target.is_self);
    }

    #[test]
    fn role_and_ssh_profile_accessors_expose_derived_values() {
        let mut host = HostEntity::new("atlas", "10.0.0.8", false);
        assert_eq!(host.role(), &HostRole::Server);
        host.role = HostRole::Desktop;
        assert_eq!(host.role(), &HostRole::Desktop);

        let via_profile = host.ssh_profile();
        assert_eq!(via_profile.user(), "root");
        assert_eq!(via_profile.port(), 22);
        assert!(!via_profile.sudo());
        assert_eq!(via_profile, SshProfile::for_host(&host));
    }

    #[test]
    fn explicit_profile_builds_and_flows_proxy_and_timeout() {
        let profile = SshProfile::new("philipp", 2222)
            .with_proxy_jump("gate.example.net")
            .with_timeout_secs(45);
        assert_eq!(profile.user(), "philipp");
        assert_eq!(profile.port(), 2222);
        assert_eq!(profile.proxy_jump(), Some("gate.example.net"));
        assert_eq!(profile.timeout_secs(), 45);
        assert_eq!(profile.identity_file(), None);
    }

    #[test]
    fn user_port_and_sudo_builders_return_new_profiles() {
        let base = SshProfile::new("philipp", 2222);
        let changed = base
            .clone()
            .with_user("deploy")
            .with_port(2200)
            .with_sudo(false);
        // The original is untouched; the new profile carries the overrides.
        assert_eq!(base.user(), "philipp");
        assert_eq!(base.port(), 2222);
        assert!(base.sudo());
        assert_eq!(changed.user(), "deploy");
        assert_eq!(changed.port(), 2200);
        assert!(!changed.sudo());
    }

    #[test]
    fn tag_membership_and_role_matching() {
        let mut host = HostEntity::new("atlas", "10.0.0.8", false);
        assert!(!host.has_tag("prod"));
        host.tags = vec!["prod".to_string(), "web".to_string()];
        assert!(host.has_tag("prod"));
        assert!(host.has_tag("web"));
        assert!(!host.has_tag("dev"));

        assert!(host.matches_role("server"));
        assert!(host.matches_role("SERVER"));
        host.role = HostRole::parse("desktop");
        assert!(host.matches_role("desktop"));
        assert!(!host.matches_role("server"));
    }
}
