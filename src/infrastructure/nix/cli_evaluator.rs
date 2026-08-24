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

    /// Resolves the flake root to an absolute path for downstream Nix
    /// expressions (`import "<abs>"`, `#nixosConfigurations`). A bare
    /// relative path such as `.` would otherwise embed as `import "."`,
    /// which Lix rejects with "string '.' doesn't represent an absolute
    /// path". Errors when the path cannot be canonicalized (missing root).
    fn canonical_flake(flake_path: &Path) -> Result<PathBuf, NodError> {
        std::fs::canonicalize(flake_path).map_err(|e| NodError::Config {
            detail: format!(
                "cannot resolve flake path '{}': {}",
                flake_path.display(),
                e
            ),
        })
    }

    /// Builds the `--apply` lambda that extracts the `config.nod` surface
    /// (AC4, tier 3) from a host's evaluated config. The host name is escaped
    /// so names containing quotes or `${` evaluate literally. It reads a
    /// single `x` argument (the evaluated config) and returns the same field
    /// set node's `NixCliEvaluator::collect_hosts` deserializes.
    ///
    /// No `import` is emitted: the host config is reached via a flake
    /// reference (`<path>#nixosConfigurations.<name>.config`), which is
    /// pure-mode-safe and works for flakes without a `default.nix`.
    fn build_meta_expr(flake_path: &Path, name: &str) -> String {
        let _ = flake_path; // kept for signature stability / escaping context
        let name = Self::nix_escape(name);
        format!(
            "x: {{ targetHost = if x ? nod && x.nod ? targetHost then x.nod.targetHost else (if x ? deployment && x.deployment ? targetHost then x.deployment.targetHost else (if x ? networking && x.networking ? hostName then x.networking.hostName else \"{name}\")); role = if x ? nod && x.nod ? role then x.nod.role else (if x ? deployment && x.deployment ? role then x.deployment.role else \"server\"); tags = if x ? nod && x.nod ? tags && builtins.isList x.nod.tags then map toString x.nod.tags else []; user = if x ? nod && x.nod ? ssh && x.nod.ssh ? user then x.nod.ssh.user else (if x ? nod && x.nod ? user then x.nod.user else null); port = if x ? nod && x.nod ? ssh && x.nod.ssh ? port && builtins.isInt x.nod.ssh.port then x.nod.ssh.port else (if x ? nod && x.nod ? port && builtins.isInt x.nod.port then x.nod.port else null); nod = if x ? nod then x.nod else null; }}",
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

    /// Runs one host's `nix eval` subprocess for its `config.nod` metadata and
    /// maps the outcome through [`Self::eval_meta`]. The subprocess launch is
    /// kept here (async) so the pure [`Self::collect_hosts`] fold stays
    /// unit-testable without a Nix toolchain.
    ///
    /// The host config is reached via a flake reference
    /// (`<abs-path>#nixosConfigurations.<name>.config`) with an `--apply`
    /// field-extraction lambda — pure-mode-safe and correct for flakes that
    /// have no `default.nix` (an `import "<path>"` approach fails on both
    /// counts).
    async fn eval_host_meta(&self, flake_path: &Path, name: &str) -> Result<FlakeMeta, NodError> {
        let lambda = Self::build_meta_expr(flake_path, name);
        let flake_ref = format!(
            "{}#nixosConfigurations.{}.config",
            flake_path.display(),
            name
        );
        let args = Self::meta_eval_args(&flake_ref, &lambda);
        let meta_output = Command::new("nix").args(&args).output().await;
        Self::eval_meta(name, meta_output)
    }

    /// Builds the `nix eval` argv for one host's metadata: evaluate the flake
    /// reference and map it through the `--apply` field-extraction lambda.
    /// Pure and side-effect free so the command shape is unit-testable
    /// without a Nix toolchain.
    fn meta_eval_args(flake_ref: &str, lambda: &str) -> Vec<String> {
        vec![
            "eval".to_string(),
            "--json".to_string(),
            "--apply".to_string(),
            lambda.to_string(),
            flake_ref.to_string(),
        ]
    }

    /// Pure per-host fold: converts each per-host metadata result into a
    /// `HostEntity`, carrying any per-host eval/parse failure as
    /// `Err((name, error))` so the caller decides whether to hard-fail
    /// (strict) or skip-and-report (degraded). No subprocesses here, so the
    /// entities and the failure pairing are unit-testable in isolation.
    fn collect_hosts(
        meta_results: Vec<(String, Result<FlakeMeta, NodError>)>,
    ) -> Vec<Result<HostEntity, (String, NodError)>> {
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();
        meta_results
            .into_iter()
            .map(|(name, meta)| match meta {
                Ok(meta) => {
                    let mut entity =
                        HostEntity::new(&name, &meta.target_host, name == local_hostname);
                    entity.role = HostRole::parse(&meta.role);
                    entity.tags = meta.tags.clone();
                    if let Some(user) = meta.user {
                        entity.target_user = user;
                    }
                    if let Some(port) = meta.port {
                        entity.target_port = port;
                    }
                    // Materialize the full `config.nod` surface (tier 3) so
                    // downstream adapters read the granular
                    // ssh/build/rollout/health/hooks values.
                    if let Some(nod) = meta.nod {
                        entity.nod_config = nod;
                    }
                    Ok(entity)
                }
                Err(e) => Err((name, e)),
            })
            .collect()
    }

    /// Builds the operator-facing warning for a skipped host, naming the host
    /// so the operator knows which fleet member was silently omitted (AC6).
    fn skip_warning(name: &str, err: &NodError) -> String {
        format!("warning: skipping host '{name}': {err}")
    }

    /// Resolves the per-host results into discovered hosts. In degraded mode a
    /// failing host is skipped with a warning naming it; in strict mode any
    /// failure is propagated as a hard error (which already carries the host
    /// name from [`Self::eval_meta`]).
    fn resolve_hosts(
        results: Vec<Result<HostEntity, (String, NodError)>>,
        degraded: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        let mut hosts = Vec::with_capacity(results.len());
        for res in results {
            match res {
                Ok(host) => hosts.push(host),
                Err((name, e)) if degraded => {
                    eprintln!("{}", Self::skip_warning(&name, &e));
                }
                Err((_, e)) => return Err(e),
            }
        }
        Ok(hosts)
    }

    /// Runs the whole-matrix `nix eval` (host name list) plus each per-host
    /// metadata eval, returning the per-host results. The whole-matrix failure
    /// is hard in both strict and degraded modes (AC2); only per-host
    /// failures are deferred to [`Self::resolve_hosts`].
    async fn eval_host_metas(
        &self,
        flake_path: &Path,
    ) -> Result<Vec<(String, Result<FlakeMeta, NodError>)>, NodError> {
        // Canonicalize the flake path once so downstream references
        // (`import "<abs>"`, `#nixosConfigurations`) are absolute. A relative
        // path such as `.` otherwise embeds as `import "."`, which Lix rejects
        // with "string '.' doesn't represent an absolute path".
        let flake_path = Self::canonical_flake(flake_path)?;
        let pb = Self::create_braille_spinner("Evaluating host matrix...");
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

        let mut meta_results = Vec::with_capacity(host_names.len());
        for name in host_names {
            let meta = self.eval_host_meta(flake_path.as_path(), &name).await;
            meta_results.push((name, meta));
        }
        Ok(meta_results)
    }
}

#[async_trait]
impl EvaluatorPort for NixCliEvaluator {
    async fn discover_hosts(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        // Kept for compatibility / the port contract; the default behaviour is
        // strict (any per-host failure is a hard error), matching the legacy
        // semantics this method always had.
        self.discover_hosts_strict(flake_path, verbose).await
    }

    async fn discover_hosts_strict(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        let start = Instant::now();
        let meta_results = self.eval_host_metas(flake_path).await?;
        let results = Self::collect_hosts(meta_results);
        // Strict: the first per-host metadata eval/parse failure is a hard
        // error carrying the failing host's name (AC3).
        let hosts = Self::resolve_hosts(results, false)?;
        if verbose {
            println!(
                "  {}",
                format!("Discovered {} hosts in {:?}", hosts.len(), start.elapsed()).dimmed()
            );
        }
        Ok(hosts)
    }

    async fn discover_hosts_degraded(
        &self,
        flake_path: &Path,
        verbose: bool,
    ) -> Result<Vec<HostEntity>, NodError> {
        let start = Instant::now();
        let meta_results = self.eval_host_metas(flake_path).await?;
        let results = Self::collect_hosts(meta_results);
        // Degraded: a failing host's metadata is skipped and reported via a
        // warning naming it, while the rest of the fleet is still returned
        // (AC2/AC6). Only a whole-matrix failure (already surfaced by
        // `eval_host_metas`) hard-fails.
        let hosts = Self::resolve_hosts(results, true)?;
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
    fn canonical_flake_resolves_relative_to_absolute() {
        // Regression: `nod switch` defaults the flake path to `.`, which was
        // embedded verbatim as `import "."` and rejected by Lix with "string
        // '.' doesn't represent an absolute path".
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let abs = NixCliEvaluator::canonical_flake(Path::new(".")).unwrap();
        std::env::set_current_dir(cwd).unwrap();
        assert!(abs.is_absolute());
        assert_eq!(abs, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn canonical_flake_errors_on_missing_root() {
        let missing = Path::new("/definitely/not/a/real/flake/dir");
        let err = NixCliEvaluator::canonical_flake(missing).unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[test]
    fn meta_eval_args_use_flake_ref_and_apply() {
        // The per-host metadata is reached via a flake reference with an
        // `--apply` field-extraction lambda (no `import`, no `--impure`),
        // which is pure-mode-safe and works for flakes without `default.nix`.
        let args =
            NixCliEvaluator::meta_eval_args("/etc/nixos#nixosConfigurations.yorke.config", "x: x");
        assert_eq!(args[0], "eval");
        assert!(args.iter().any(|a| a == "--json"));
        assert!(args.iter().any(|a| a == "--apply"));
        assert!(args.contains(&"x: x".to_string()));
        assert!(args.contains(&"/etc/nixos#nixosConfigurations.yorke.config".to_string()));
        // No `--impure` and no `--expr`-embedded `import` — pure-mode-safe.
        assert!(!args.iter().any(|a| a == "--impure"));
    }

    #[test]
    fn meta_expr_is_a_field_extraction_lambda_not_an_import() {
        // The meta expression is an `--apply` lambda, not a `let ... import
        // "<path>"` expression — `import` of a bare dir requires a
        // `default.nix` and is forbidden in pure mode, both of which break
        // flake-based discovery.
        let expr = NixCliEvaluator::build_meta_expr(Path::new("/tmp/my flake"), "edge\"host");
        assert!(expr.starts_with("x: {"), "must be a lambda, got: {expr}");
        assert!(!expr.contains("import "));
        assert!(!expr.contains("let x ="));
        assert!(expr.contains("else \"edge\\\"host\""));
    }

    #[test]
    fn meta_expr_terminates_every_binding_with_a_semicolon() {
        // Regression: Lix 2.95 (strict parser) requires the final attrset
        // binding to be terminated with `;` before the closing brace. The
        // generated meta expression used to end `nod = ... else null }` with
        // no semicolon, which evaluated on stock Nix but failed to parse on
        // Lix with "expecting ';' to end binding", skipping every host.
        let expr = NixCliEvaluator::build_meta_expr(Path::new("."), "rollins");
        // Any binding with a trailing `}` directly after a value (no `;`) is a
        // parse hazard under Lix. The whole expr is `... in { ... } ` and must
        // not contain `null }` or `] }` un-terminated patterns before the close.
        assert!(!expr.contains("null }"));
        assert!(!expr.contains("] }"));
        // The closing brace must come immediately after a `;`-terminated binding.
        let close_offset = expr.rfind('}').expect("expr closes with a brace");
        let before_close = &expr[..close_offset];
        assert!(
            before_close.trim_end().ends_with(';'),
            "the value before the closing brace must end with ';' (Lix 2.95 strict parser)"
        );
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

    /// Builds a per-host metadata result from a JSON body so tests can exercise
    /// [`NixCliEvaluator::collect_hosts`] / [`NixCliEvaluator::resolve_hosts`]
    /// without a Nix subprocess (AC5 injectable seam).
    fn meta_ok(name: &str, target: &str, role: &str) -> (String, Result<FlakeMeta, NodError>) {
        let json = format!(r#"{{"targetHost":"{target}","role":"{role}"}}"#);
        let meta = serde_json::from_str::<FlakeMeta>(&json).unwrap();
        (name.to_string(), Ok(meta))
    }

    fn meta_err(name: &str) -> (String, Result<FlakeMeta, NodError>) {
        (
            name.to_string(),
            Err(NodError::parse_failure(format!(
                "flake metadata JSON for host '{name}'"
            ))),
        )
    }

    #[test]
    fn collect_hosts_carries_failures_alongside_entities() {
        let metas = vec![
            meta_ok("atlas", "10.0.0.8", "server"),
            meta_err("broken"),
            meta_ok("juno", "10.0.0.9", "desktop"),
        ];
        let results = NixCliEvaluator::collect_hosts(metas);
        assert_eq!(results.len(), 3);
        // Successful hosts materialize as entities (in order), the failure is
        // carried with the host name for the caller to handle.
        assert!(results[0].as_ref().unwrap().name == "atlas");
        assert!(matches!(&results[1], Err((name, _)) if name == "broken"));
        assert!(results[2].as_ref().unwrap().name == "juno");
        assert_eq!(results[0].as_ref().unwrap().role.to_str(), "server");
    }

    #[test]
    fn resolve_hosts_degraded_skips_failing_host_and_keeps_successes() {
        let results = vec![
            Ok(HostEntity::new("atlas", "10.0.0.8", false)),
            Err((
                "broken".to_string(),
                NodError::parse_failure("flake metadata JSON for host 'broken'"),
            )),
            Ok(HostEntity::new("juno", "10.0.0.9", false)),
        ];
        let hosts = NixCliEvaluator::resolve_hosts(results, true).unwrap();
        // Failing host omitted; succeeding hosts returned.
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "atlas");
        assert_eq!(hosts[1].name, "juno");
    }

    #[test]
    fn resolve_hosts_strict_hard_fails_on_first_error_with_host() {
        let results = vec![
            Ok(HostEntity::new("atlas", "10.0.0.8", false)),
            Err((
                "broken".to_string(),
                NodError::parse_failure("flake metadata JSON for host 'broken'"),
            )),
            Ok(HostEntity::new("juno", "10.0.0.9", false)),
        ];
        let err = NixCliEvaluator::resolve_hosts(results, false).unwrap_err();
        assert!(err.to_string().contains("broken"));
    }

    #[test]
    fn resolve_hosts_degraded_all_success_returns_every_host() {
        let results = vec![
            Ok(HostEntity::new("atlas", "10.0.0.8", false)),
            Ok(HostEntity::new("juno", "10.0.0.9", false)),
        ];
        let hosts = NixCliEvaluator::resolve_hosts(results, true).unwrap();
        assert_eq!(hosts.len(), 2);
    }

    #[test]
    fn skip_warning_names_the_host_and_error() {
        let warning = NixCliEvaluator::skip_warning(
            "broken",
            &NodError::parse_failure("flake metadata JSON for host 'broken'"),
        );
        assert!(warning.starts_with("warning: skipping host '"));
        assert!(warning.contains("'broken'"));
        // Warning echoes the underlying per-host failure detail too (AC6).
        assert!(warning.contains("flake metadata JSON for host 'broken'"));
    }
}
