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
    ApplyRequest, ApplyResponse, DesiredResource, Handshake, HandshakeRequest, ObserveRequest,
    ObserveResponse, ObservedResource, PlanRequest, PlanResponse, PROTOCOL_VERSION,
};

/// Time the handshake may take before the provider counts as broken.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

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
    pub async fn open(command: &Path, timeout: Duration) -> Result<(Self, Handshake), NodError> {
        if !command.exists() {
            return Err(NodError::config(format!(
                "provider command does not exist: {}",
                command.display()
            )));
        }

        let mut child = Command::new(command)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| {
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

    /// The provider's executable path.
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
        desired: &[DesiredResource],
        observed: &[ObservedResource],
    ) -> Result<PlanResponse, NodError> {
        self.exchange(
            &PlanRequest {
                op: "plan",
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
        changes: &[crate::domain::provider::Change],
    ) -> Result<ApplyResponse, NodError> {
        self.exchange(
            &ApplyRequest {
                op: "apply",
                changes,
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

        let mut response = String::new();
        let read = tokio::time::timeout(timeout, self.stdout.read_line(&mut response)).await;
        match read {
            Ok(Ok(0)) => Err(NodError::provider(format!(
                "provider {} closed the connection",
                self.command.display()
            ))),
            Ok(Ok(_)) => {
                let value: serde_json::Value =
                    serde_json::from_str(response.trim()).map_err(|error| {
                        NodError::provider(format!(
                            "provider {} sent invalid JSON: {error}",
                            self.command.display()
                        ))
                    })?;

                // An error is not an empty answer: a provider that cannot
                // authenticate must say so, or `resources: []` reads as an
                // empty world.
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
            Ok(Err(error)) => Err(NodError::provider(format!(
                "cannot read from provider {}: {error}",
                self.command.display()
            ))),
            Err(_) => {
                let _ = self.child.start_kill();
                Err(NodError::provider(format!(
                    "provider {} timed out after {:?}",
                    self.command.display(),
                    timeout
                )))
            }
        }
    }
}

impl Drop for ProcessProvider {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// A request whose timeout differs from the provider's operation timeout.
trait WithTime<T> {
    fn with_time(self, timeout: Duration) -> Self;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  case \"$line\" in\n    *protocol*) echo '{\"provider\":\"fake\",\"version\":\"0.1.0\",\"protocol\":1,\"capabilities\":[],\"ownership\":\"marker\",\"settings_schema\":{}}' ;;\n    *observe*) echo '{\"resources\":[],\"conditions\":[]}' ;;\n  esac\ndone\n",
        );

        let (mut provider, handshake) = ProcessProvider::open(&path, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(handshake.provider, "fake");
        let observed = provider.observe(&serde_json::json!({})).await.unwrap();
        assert!(observed.resources.is_empty());
    }

    #[tokio::test]
    async fn a_wrong_protocol_version_is_rejected() {
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  echo '{\"provider\":\"fake\",\"version\":\"0.1.0\",\"protocol\":2,\"ownership\":\"marker\"}'\ndone\n",
        );

        let error = ProcessProvider::open(&path, Duration::from_secs(5))
            .await
            .err()
            .expect("mismatch must fail");
        assert!(error.to_string().contains("speaks protocol 2"));
    }

    #[tokio::test]
    async fn a_provider_that_logs_on_stdout_breaks_the_protocol() {
        let (_dir, path) = fake_provider(
            "#!/bin/sh\nwhile read -r line; do\n  echo 'starting up'\n  echo '{\"provider\":\"fake\",\"version\":\"0.1.0\",\"protocol\":1,\"ownership\":\"marker\"}'\ndone\n",
        );

        let error = ProcessProvider::open(&path, Duration::from_secs(5))
            .await
            .err()
            .expect("invalid JSON must fail");
        assert!(error.to_string().contains("invalid JSON"));
    }
}
