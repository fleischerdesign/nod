//! Nix CLI evaluator adapter: host discovery and closure building.

use async_trait::async_trait;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};
use tokio::process::Command;

use crate::domain::config::NodConfig;
use crate::domain::errors::NodError;
use crate::domain::host::{BuilderHost, HostEntity, HostRole};
use crate::domain::ports::evaluator::EvaluatorPort;
use serde::Deserialize;

/// Parsed per-host flake metadata (`config.nod`) with graceful fallbacks
/// (ADR-004 tier 3). JSON keys are camelCase via serde renaming; the nested
/// `nod` object carries the whole module surface (ssh / build / rollout /
/// health / hooks) and is materialized onto the `HostEntity`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlakeMeta {
    target_host: String,
    role: String,
    #[serde(default)]
    tags: Vec<String>,
    user: Option<String>,
    port: Option<u16>,
    /// The raw `config.nod` object; `null` when the host does not use the
    /// nod module.
    #[serde(default)]
    nod: Option<NodConfig>,
}

/// Evaluates the flake via the Nix CLI to discover hosts and build closures.
pub struct NixCliEvaluator;

impl NixCliEvaluator {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NixCliEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl NixCliEvaluator {
    fn create_braille_spinner(msg: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("  {spinner:.cyan} {msg}")
                .unwrap(),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    }

    /// Escapes a value for interpolation into a double-quoted Nix string
    /// literal (AC4): `\`, `"` and the `${` interpolation marker are escaped
    /// so host names / flake paths with special characters evaluate literally
    /// (spaces need no escaping inside a Nix string).
    fn nix_escape(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '$' if chars.peek() == Some(&'{') => {
                    out.push_str("\\${");
                    chars.next(); // consume the '{'
                }
                _ => out.push(c),
            }
        }
        out
    }

    /// Builds the Nix expression that evaluates one host's `config.nod`
    /// surface (AC4, tier 3). The flake path and host name are escaped as
    /// Nix string literals (and the host selected with a quoted attribute) so
    /// paths/names containing spaces, quotes or `${` evaluate correctly.
    fn build_meta_expr(flake_path: &Path, name: &str) -> String {
        let path = Self::nix_escape(&flake_path.display().to_string());
        let name = Self::nix_escape(name);
        format!(
            "let x = (import \"{path}\").nixosConfigurations.\"{name}\".config; in {{ targetHost = if x ? nod && x.nod ? targetHost then x.nod.targetHost else (if x ? deployment && x.deployment ? targetHost then x.deployment.targetHost else (if x ? networking && x.networking ? hostName then x.networking.hostName else \"{name}\")); role = if x ? nod && x.nod ? role then x.nod.role else (if x ? deployment && x.deployment ? role then x.deployment.role else \"server\"); tags = if x ? nod && x.nod ? tags && builtins.isList x.nod.tags then map toString x.nod.tags else []; user = if x ? nod && x.nod ? ssh && x.nod.ssh ? user then x.nod.ssh.user else (if x ? nod && x.nod ? user then x.nod.user else null); port = if x ? nod && x.nod ? ssh && x.nod.ssh ? port && builtins.isInt x.nod.ssh.port then x.nod.ssh.port else (if x ? nod && x.nod ? port && builtins.isInt x.nod.port then x.nod.port else null); nod = if x ? nod then x.nod else null }} ",
        )
    }

    /// Runs one per-host `nix eval` and parses its JSON into `FlakeMeta`,
    /// propagating spawn, non-zero exit and parse failures as typed errors
    /// keyed on the host name (AC4) instead of silently defaulting to
    /// `FlakeMeta::default()`.
    fn eval_meta(
        name: &str,
        output: Result<Output, std::io::Error>,
    ) -> Result<FlakeMeta, NodError> {
        let output = output.map_err(|_| {
            NodError::evaluation(format!(
                "failed to evaluate flake metadata for host '{name}': could not launch `nix eval`"
            ))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::evaluation(format!(
                "failed to evaluate flake metadata for host '{name}': {}",
                stderr.trim()
            )));
        }
        serde_json::from_slice::<FlakeMeta>(&output.stdout)
            .map_err(|_| NodError::parse_failure(format!("flake metadata JSON for host '{name}'")))
    }
}

#[async_trait]
impl EvaluatorPort for NixCliEvaluator {
    async fn discover_hosts(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        let pb = Self::create_braille_spinner("Evaluating host matrix...");
        let start = Instant::now();

        let output = Command::new("nix")
            .args([
                "eval",
                "--json",
                &format!("{}#nixosConfigurations", flake_path.display()),
                "--apply",
                "builtins.attrNames",
            ])
            .output()
            .await;

        pb.finish_and_clear();

        if output.is_err() {
            return Err(NodError::discovery_failure("failed to launch `nix eval`"));
        }
        let output = output.unwrap();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::discovery_failure(stderr));
        }

        let parsed = serde_json::from_slice::<Vec<String>>(&output.stdout);
        if parsed.is_err() {
            return Err(NodError::parse_failure("host discovery JSON"));
        }
        let host_names = parsed.unwrap();

        let mut hosts = Vec::new();
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();

        for name in host_names {
            let is_local = name == local_hostname;

            // Per-host metadata from flake `config.nod` (tier 3), falling back
            // to `deployment.*` / `networking.hostName` via the Nix expression
            // (ADR-004). The whole `nod` object is emitted so every granular
            // option (ssh/build/rollout/healthChecks/hooks) deserializes onto
            // the `HostEntity`. A failed eval/parse propagates as a typed error
            // (AC4) rather than silently defaulting `FlakeMeta`.
            let meta_expr = Self::build_meta_expr(flake_path, &name);
            let meta_output = Command::new("nix")
                .args(["eval", "--json", "--expr", &meta_expr])
                .output()
                .await;
            let meta = Self::eval_meta(&name, meta_output)?;

            let mut entity = HostEntity::new(&name, &meta.target_host, is_local);
            entity.role = HostRole::parse(&meta.role);
            entity.tags = meta.tags.clone();
            if let Some(user) = meta.user {
                entity.target_user = user;
            }
            if let Some(port) = meta.port {
                entity.target_port = port;
            }
            // Materialize the full `config.nod` surface (tier 3) so downstream
            // adapters read the granular ssh/build/rollout/health/hooks values.
            if let Some(nod) = meta.nod {
                entity.nod_config = nod;
            }
            hosts.push(entity);
        }

        if verbose {
            println!(
                "  {}",
                format!("Discovered {} hosts in {:?}", hosts.len(), start.elapsed()).dimmed()
            );
        }

        Ok(hosts)
    }

    async fn build_toplevel<'a>(
        &self,
        flake_path: &Path,
        host_name: &str,
        builder: Option<&'a BuilderHost>,
        verbose: bool,
    ) -> Result<PathBuf, NodError> {
        let pb = Self::create_braille_spinner(&format!("Building closure for {}...", host_name));

        let flake_attr = format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            flake_path.display(),
            host_name
        );

        let start = Instant::now();

        // Remote build: compile the closure on the selected builder host over
        // the Nix `--builders` SSH transport (`ssh://<user>@<host>[:<port>]`).
        // A plain local build keeps the SSH-specific flags out of the vector.
        let mut args = Vec::<String>::new();
        args.push("build".to_string());
        args.push("--json".to_string());
        args.push(flake_attr.clone());
        if let Some(b) = builder {
            args.push("--builders".to_string());
            args.push(build_builder_uri(b));
        }
        args.push("--no-link".to_string());

        let output = Command::new("nix").args(&args).output().await;

        pb.finish_and_clear();

        if output.is_err() {
            return Err(NodError::build_failure(
                host_name,
                "failed to launch `nix build`",
            ));
        }
        let output = output.unwrap();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::build_failure(host_name, stderr));
        }

        let build_json = serde_json::from_slice::<serde_json::Value>(&output.stdout);
        if build_json.is_err() {
            return Err(NodError::parse_failure("nix build JSON"));
        }
        let build_json = build_json.unwrap();

        let out_path = build_json[0]["outputs"]["out"]
            .as_str()
            .ok_or_else(|| NodError::parse_failure("build JSON contained no `out` store path"))?;

        if verbose {
            println!(
                "  {}",
                format!("Build completed in {:?} -> {}", start.elapsed(), out_path).dimmed()
            );
        }

        Ok(PathBuf::from(out_path))
    }
}

/// Builds the Nix `--builders` SSH transport URI for a builder host
/// (`ssh://<user>@<target_host>`, appending `:<port>` when not 22).
fn build_builder_uri(b: &BuilderHost) -> String {
    let port = b.profile.port();
    let authority = if port == 22 {
        format!("{}@{}", b.profile.user(), b.target_host)
    } else {
        format!("{}@{}:{}", b.profile.user(), b.target_host, port)
    };
    format!("ssh://{}", authority)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::SshProfile;

    #[test]
    fn unit_struct_is_constructible_and_send_sync() {
        let evaluator = NixCliEvaluator::new();
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NixCliEvaluator>();
        let _ = evaluator;
    }

    #[test]
    fn flake_meta_deserializes_camel_case_fields() {
        let json = r#"{"targetHost":"10.0.0.8","role":"server","tags":["rack","prod"],"user":"root","port":2222}"#;
        let meta = serde_json::from_str::<FlakeMeta>(json).unwrap();
        assert_eq!(meta.target_host, "10.0.0.8");
        assert_eq!(meta.role, "server");
        assert_eq!(meta.tags, vec!["rack".to_string(), "prod".to_string()]);
        assert_eq!(meta.user, Some("root".to_string()));
        assert_eq!(meta.port, Some(2222));
    }

    #[test]
    fn flake_meta_tolerates_missing_optional_fields() {
        let json = r#"{"targetHost":"jello-machine","role":"desktop"}"#;
        let meta = serde_json::from_str::<FlakeMeta>(json).unwrap();
        assert_eq!(meta.target_host, "jello-machine");
        assert_eq!(meta.role, "desktop");
        assert!(meta.tags.is_empty());
        assert!(meta.user.is_none());
        assert!(meta.port.is_none());
    }

    #[test]
    fn builder_uri_defaults_to_root_user_and_no_port() {
        let host = HostEntity::new("atlas", "10.0.0.8", false);
        let builder = BuilderHost {
            target_host: host.target_host.clone(),
            profile: SshProfile::for_host(&host),
        };
        assert_eq!(build_builder_uri(&builder), "ssh://root@10.0.0.8");
    }

    #[test]
    fn builder_uri_appends_port_when_custom() {
        let builder = BuilderHost {
            target_host: "buildy".to_string(),
            profile: SshProfile::new("root", 2222),
        };
        assert_eq!(build_builder_uri(&builder), "ssh://root@buildy:2222");
    }

    #[test]
    fn builder_uri_custom_user_without_port() {
        let builder = BuilderHost {
            target_host: "atlas".to_string(),
            profile: SshProfile::new("deploy", 22),
        };
        assert_eq!(build_builder_uri(&builder), "ssh://deploy@atlas");
    }

    #[test]
    fn builder_uri_custom_user_and_port_combined() {
        let builder = BuilderHost {
            target_host: "buildy".to_string(),
            profile: SshProfile::new("deploy", 2200),
        };
        assert_eq!(build_builder_uri(&builder), "ssh://deploy@buildy:2200");
    }

    #[test]
    fn builder_uri_identity_uses_default_ssh_scheme() {
        // The identity-to-local path leaves the builder unset (flag-only
        // handling in the command); the URI form itself always carries a host.
        let builder = BuilderHost {
            target_host: "jello".to_string(),
            profile: SshProfile::for_host(&HostEntity::new("jello", "jello-machine", true)),
        };
        assert_eq!(build_builder_uri(&builder), "ssh://root@jello");
    }

    #[test]
    fn nix_escape_handles_quotes_backslashes_and_interpolation() {
        assert_eq!(NixCliEvaluator::nix_escape("plain-name"), "plain-name");
        assert_eq!(NixCliEvaluator::nix_escape("a\"b"), "a\\\"b");
        assert_eq!(NixCliEvaluator::nix_escape("a\\b"), "a\\\\b");
        assert_eq!(NixCliEvaluator::nix_escape("${x}"), "\\${x}");
        // Spaces need no escaping inside a Nix string literal.
        assert_eq!(
            NixCliEvaluator::nix_escape("host with spaces"),
            "host with spaces"
        );
    }

    #[test]
    fn meta_expr_escapes_path_and_name() {
        let expr = NixCliEvaluator::build_meta_expr(Path::new("/tmp/my flake"), "edge\"host");
        assert!(expr.contains("(import \"/tmp/my flake\")"));
        assert!(expr.contains("nixosConfigurations.\"edge\\\"host\""));
        assert!(expr.contains("else \"edge\\\"host\""));
    }

    #[test]
    fn failing_eval_spawn_yields_err_with_host_name() {
        let err = NixCliEvaluator::eval_meta(
            "atlas",
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "nix missing",
            )),
        )
        .unwrap_err();
        assert!(matches!(err, NodError::Evaluation { .. }));
        assert!(err.to_string().contains("atlas"));
    }

    #[test]
    fn failing_eval_exit_yields_err() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 1")
            .status()
            .unwrap();
        let output = Output {
            status,
            stdout: Vec::new(),
            stderr: b"boom".to_vec(),
        };
        let err = NixCliEvaluator::eval_meta("atlas", Ok(output)).unwrap_err();
        assert!(matches!(err, NodError::Evaluation { .. }));
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn unparseable_eval_json_yields_parse_failure() {
        let output = Output {
            status: std::process::ExitStatus::default(),
            stdout: b"not json".to_vec(),
            stderr: Vec::new(),
        };
        let err = NixCliEvaluator::eval_meta("atlas", Ok(output)).unwrap_err();
        assert!(matches!(err, NodError::Evaluation { .. }));
        assert!(err.to_string().contains("atlas"));
    }

    #[test]
    fn eval_meta_parses_valid_json() {
        let json = br#"{"targetHost":"10.0.0.8","role":"server"}"#;
        let output = Output {
            status: std::process::ExitStatus::default(),
            stdout: json.to_vec(),
            stderr: Vec::new(),
        };
        let meta = NixCliEvaluator::eval_meta("atlas", Ok(output)).unwrap();
        assert_eq!(meta.target_host, "10.0.0.8");
        assert_eq!(meta.role, "server");
    }
}
