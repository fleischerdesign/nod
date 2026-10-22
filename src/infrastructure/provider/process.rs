//! Process provider supervisor (ADR-030).
//!
//! The core starts a provider, speaks JSON Lines with it and kills it when the
//! conversation ends or a timeout fires. stdout is protocol, stderr is log; a
//! non-JSON line on stdout is a protocol violation, never a warning.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::domain::errors::NodError;
use crate::domain::provider::{
    AdoptRequest, AdoptResponse, ApplyRequest, ApplyResponse, DesiredResource, Handshake,
    HandshakeRequest, ObserveRequest, ObserveResponse, ObservedResource, PlanRequest, PlanResponse,
    PROTOCOL_VERSION,
};

/// Time the handshake may take before the provider counts as broken.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Largest protocol frame the core accepts.
///
/// A line is one response; a provider that sends more than this is broken or
/// hostile, and buffering it without bound would exhaust the reconciler's
/// memory. 16 MiB holds a large zone with room to spare.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// A running provider process.
pub struct ProcessProvider {
    command: PathBuf,
    timeout: Duration,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ProcessProvider {
    /// Starts `command` and completes the handshake.
    ///
    /// `env` names the variables the provider inherits from the core's own
    /// environment. The core forwards names, never semantics: what a variable
    /// means is the provider's business. A declared variable that is not set is
    /// a configuration error named after the variable, never its value.
    pub async fn open(
        command: &Path,
        timeout: Duration,
        env: &[String],
    ) -> Result<(Self, Handshake), NodError> {
        if !command.exists() {
            return Err(NodError::config(format!(
                "provider command does not exist: {}",
                command.display()
            )));
        }

        let inherited = child_environment(env)?;

        let mut builder = Command::new(command);
        builder
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (name, value) in &inherited {
            // Forwarded by name only; nothing logs the value.
            builder.env(name, value);
        }

        let mut child = builder.spawn().map_err(|error| {
            NodError::provider(format!(
                "cannot start provider {}: {error}",
                command.display()
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| NodError::provider("provider stdin is not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| NodError::provider("provider stdout is not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| NodError::provider("provider stderr is not piped"))?;

        // Provider logs go to tracing, never into the protocol stream.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "nod_provider", "{line}");
            }
        });

        let mut provider = Self {
            command: command.to_path_buf(),
            timeout,
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };

        let handshake = provider.handshake().await?;
        Ok((provider, handshake))
    }

    /// Provider's executable path.
    pub fn command(&self) -> &Path {
        &self.command
    }

    /// Sends the handshake and validates the answer against the core's protocol.
    async fn handshake(&mut self) -> Result<Handshake, NodError> {
        let answer: Handshake = self
            .exchange(
                &HandshakeRequest {
                    protocol: PROTOCOL_VERSION,
                },
                HANDSHAKE_TIMEOUT,
            )
            .await?;

        if answer.protocol != PROTOCOL_VERSION {
            return Err(NodError::provider(format!(
                "provider {} speaks protocol {}, this nod speaks {}",
                answer.provider, answer.protocol, PROTOCOL_VERSION
            )));
        }
        Ok(answer)
    }

    /// Observes the provider's world.
    pub async fn observe(
        &mut self,
        config: &serde_json::Value,
    ) -> Result<ObserveResponse, NodError> {
        self.exchange(
            &ObserveRequest {
                op: "observe",
                config,
            },
            self.timeout,
        )
        .await
    }

    /// Asks for a plan; the core decides what may be applied.
    pub async fn plan(
        &mut self,
        config: &serde_json::Value,
        desired: &[DesiredResource],
        observed: &[ObservedResource],
    ) -> Result<PlanResponse, NodError> {
        self.exchange(
            &PlanRequest {
                op: "plan",
                config,
                desired,
                observed,
            },
            self.timeout,
        )
        .await
    }

    /// Applies exactly the changes the core approved.
    pub async fn apply(
        &mut self,
        config: &serde_json::Value,
        changes: &[crate::domain::provider::Change],
    ) -> Result<ApplyResponse, NodError> {
        self.exchange(
            &ApplyRequest {
                op: "apply",
                config,
                changes,
            },
            self.timeout,
        )
        .await
    }

    /// Stamps the provider's ownership marker on existing resources.
    ///
    /// Only providers that declare the `adopt` operation implement it; the
    /// core checks that before calling.
    pub async fn adopt(
        &mut self,
        config: &serde_json::Value,
        resources: &[ObservedResource],
    ) -> Result<AdoptResponse, NodError> {
        self.exchange(
            &AdoptRequest {
                op: "adopt",
                config,
                resources,
            },
            self.timeout,
        )
        .await
    }

    /// Writes one request line and reads one response line.
    async fn exchange<T: Serialize, R: DeserializeOwned>(
        &mut self,
        request: &T,
        timeout: Duration,
    ) -> Result<R, NodError> {
        let mut line = serde_json::to_string(request)
            .map_err(|error| NodError::provider(format!("cannot serialise request: {error}")))?;
        line.push('\n');
        if std::env::var_os("NOD_PROVIDER_TRACE").is_some() {
            eprintln!("→ {}", line.trim_end());
        }
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| {
                NodError::provider(format!(
                    "cannot write to provider {}: {error}",
                    self.command.display()
                ))
            })?;
        self.stdin.flush().await.map_err(|error| {
            NodError::provider(format!(
                "cannot flush provider {}: {error}",
                self.command.display()
            ))
        })?;

        let mut response = match tokio::time::timeout(timeout, self.read_frame()).await {
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None)) => {
                return Err(NodError::provider(format!(
                    "provider {} closed the connection",
                    self.command.display()
                )))
            }
            Ok(Err(error)) => {
                let _ = self.child.start_kill();
                return Err(error);
            }
            Err(_) => {
                let _ = self.child.start_kill();
                return Err(NodError::provider(format!(
                    "provider {} timed out after {:?}",
                    self.command.display(),
                    timeout
                )));
            }
        };

        if std::env::var_os("NOD_PROVIDER_TRACE").is_some() {
            eprintln!("← {}", String::from_utf8_lossy(&response).trim_end());
        }

        // A frame is a line; a trailing newline is framing, not content.
        if response.last() == Some(&b'\n') {
            response.pop();
        }

        let value: serde_json::Value = serde_json::from_slice(&response).map_err(|error| {
            NodError::provider(format!(
                "provider {} sent invalid JSON: {error}",
                self.command.display()
            ))
        })?;

        // An error is not an empty answer: a provider that cannot authenticate
        // must say so, or `resources: []` reads as an empty world.
        if let Some(error) = value.get("error") {
            let kind = error
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("protocol");
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("provider reported an error");
            return Err(match kind {
                "auth" => NodError::auth(message),
                "config" => NodError::config(message),
                "rate-limited" => NodError::rate_limited(
                    message,
                    error
                        .get("retry_after_secs")
                        .and_then(serde_json::Value::as_u64),
                ),
                _ => NodError::provider(format!("{kind}: {message}")),
            });
        }

        serde_json::from_value(value).map_err(|error| {
            NodError::provider(format!(
                "provider {} sent an invalid response: {error}",
                self.command.display()
            ))
        })
    }

    /// Reads one newline-terminated frame, up to [`MAX_FRAME_BYTES`].
    ///
    /// `Ok(None)` is a clean end of stream before the frame starts; a partial
    /// frame at end of stream is a truncation, not a response.
    async fn read_frame(&mut self) -> Result<Option<Vec<u8>>, NodError> {
        let mut frame = Vec::new();
        loop {
            let (newline, consumed) = {
                let available = self.stdout.fill_buf().await.map_err(|error| {
                    NodError::provider(format!(
                        "cannot read from provider {}: {error}",
                        self.command.display()
                    ))
                })?;

                if available.is_empty() {
                    if frame.is_empty() {
                        return Ok(None);
                    }
                    return Err(NodError::provider(format!(
                        "provider {} ended its output in the middle of a frame",
                        self.command.display()
                    )));
                }

                match available.iter().position(|byte| *byte == b'\n') {
                    Some(position) => {
                        frame.extend_from_slice(&available[..=position]);
                        (true, position + 1)
                    }
                    None => {
                        frame.extend_from_slice(available);
                        (false, available.len())
                    }
                }
            };
            self.stdout.consume(consumed);

            if newline {
                return Ok(Some(frame));
            }
            if frame.len() > MAX_FRAME_BYTES {
                return Err(NodError::provider(format!(
                    "provider {} exceeded the {} byte frame limit",
                    self.command.display(),
                    MAX_FRAME_BYTES
                )));
            }
        }
    }
}

impl Drop for ProcessProvider {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Resolves the variables a provider inherits, by name.
///
/// Fails on an invalid name and on a declared variable that is not set: a
/// provider that silently runs without its credential would report an empty
/// world instead of an auth error.
fn child_environment(names: &[String]) -> Result<Vec<(String, String)>, NodError> {
    let mut values = Vec::new();
    for name in names {
        if name.is_empty() || name.contains('=') {
            return Err(NodError::config(format!(
                "invalid environment variable name `{name}`"
            )));
        }
        match std::env::var(name) {
            Ok(value) => values.push((name.clone(), value)),
            Err(_) => {
                return Err(NodError::config(format!(
                    "provider requires environment variable {name}, which is not set"
                )))
            }
        }
    }
    Ok(values)
}

/// A request whose timeout differs from the provider's operation timeout.
trait WithTime<T> {
    fn with_time(self, timeout: Duration) -> Self;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// These tests write an executable and spawn it. In parallel, one test's
    /// `fork` can inherit another's open write descriptor and make the next
    /// `exec` fail with ETXTBSY. Store paths are immutable, so this is a
    /// harness artefact: serialise the tests that spawn a written script.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn fake_provider(script: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-provider.sh");
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn handshake_and_observe_round_trip() {
        let _guard = SERIAL.lock().await;
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  case \"$line\" in\n    *protocol*) echo '{\"provider\":\"fake\",\"version\":\"0.1.0\",\"protocol\":1,\"capabilities\":[],\"ownership\":\"marker\",\"settings_schema\":{}}' ;;\n    *observe*) echo '{\"resources\":[],\"conditions\":[]}' ;;\n  esac\ndone\n",
        );

        let (mut provider, handshake) = ProcessProvider::open(&path, Duration::from_secs(5), &[])
            .await
            .unwrap();
        assert_eq!(handshake.provider, "fake");
        let observed = provider.observe(&serde_json::json!({})).await.unwrap();
        assert!(observed.resources.is_empty());
    }

    #[tokio::test]
    async fn a_declared_environment_variable_reaches_the_provider() {
        let _guard = SERIAL.lock().await;
        std::env::set_var("NOD_TEST_FORWARDED", "s3cret");
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  echo \"{\\\"provider\\\":\\\"fake\\\",\\\"version\\\":\\\"$NOD_TEST_FORWARDED\\\",\\\"protocol\\\":1,\\\"ownership\\\":\\\"marker\\\"}\"\ndone\n",
        );

        let (_provider, handshake) = ProcessProvider::open(
            &path,
            Duration::from_secs(5),
            &["NOD_TEST_FORWARDED".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(handshake.version, "s3cret");
    }

    #[tokio::test]
    async fn a_missing_declared_environment_variable_is_a_config_error() {
        let _guard = SERIAL.lock().await;
        std::env::remove_var("NOD_TEST_ABSENT");
        let (_dir, path) = fake_provider("#!/bin/sh\nexit 0\n");

        let error = ProcessProvider::open(
            &path,
            Duration::from_secs(5),
            &["NOD_TEST_ABSENT".to_string()],
        )
        .await
        .err()
        .expect("a missing credential must fail before the provider starts");
        assert!(matches!(error, NodError::Config { .. }));
        assert!(error.to_string().contains("NOD_TEST_ABSENT"));
    }

    #[tokio::test]
    async fn an_oversized_frame_is_rejected() {
        let _guard = SERIAL.lock().await;
        let (_dir, path) =
            fake_provider("#!/bin/sh\nread -r line\nhead -c 17000000 /dev/zero | tr '\\0' 'x'\n");

        let error = ProcessProvider::open(&path, Duration::from_secs(20), &[])
            .await
            .err()
            .expect("a frame over the limit must fail");
        assert!(error.to_string().contains("frame limit"), "{error}");
    }

    #[tokio::test]
    async fn a_wrong_protocol_version_is_rejected() {
        let _guard = SERIAL.lock().await;
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  echo '{\"provider\":\"fake\",\"version\":\"0.1.0\",\"protocol\":2,\"ownership\":\"marker\"}'\ndone\n",
        );

        let error = ProcessProvider::open(&path, Duration::from_secs(5), &[])
            .await
            .err()
            .expect("mismatch must fail");
        assert!(error.to_string().contains("speaks protocol 2"));
    }

    #[tokio::test]
    async fn a_provider_that_logs_on_stdout_breaks_the_protocol() {
        let _guard = SERIAL.lock().await;
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  echo 'starting up'\n  echo '{\"provider\":\"fake\",\"version\":\"0.1.0\",\"protocol\":1,\"ownership\":\"marker\"}'\ndone\n",
        );

        let error = ProcessProvider::open(&path, Duration::from_secs(5), &[])
            .await
            .err()
            .expect("invalid JSON must fail");
        assert!(error.to_string().contains("invalid JSON"));
    }
}
