//! Strongly-typed error hierarchy for the nod engine.
//!
//! `NodError` is the single error seam that crosses the Domain -> Application
//! and Application -> Presentation boundaries (ADR-002). Control flow that
//! needs to *branch* on a failure class (rollback decision, fleet error
//! recovery) matches the outer variant, while the operator-facing message
//! lives in `detail`.

use thiserror::Error;

/// Root typed error class for every failed nod operation.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum NodError {
    /// Nix evaluation / host discovery / closure build failures.
    #[error("evaluation error: {detail}")]
    Evaluation { detail: String },
    /// Closure transfer / switch activation / rollback failures.
    #[error("deployment error: {detail}")]
    Deployment { detail: String },
    /// CLI, `.nod.toml`, flake metadata and defaults resolution failures.
    #[error("configuration error: {detail}")]
    Config { detail: String },
    /// Reachability and post-activation verification failures.
    #[error("health check error: {detail}")]
    HealthCheck { detail: String },
    /// Invariant / programming errors that should never surface in practice.
    #[error("internal error: {detail}")]
    Internal { detail: String },
}

impl NodError {
    /// Raised when Nix host discovery evaluation fails.
    pub fn discovery_failure(detail: impl Into<String>) -> Self {
        NodError::evaluation(format!(
            "Nix host discovery evaluation failed: {}",
            detail.into()
        ))
    }

    /// Raised when parsing Nix evaluation / build output fails.
    pub fn parse_failure(detail: impl Into<String>) -> Self {
        NodError::evaluation(format!("failed to parse Nix output: {}", detail.into()))
    }

    /// Raised when `nix build` of a host toplevel closure fails.
    pub fn build_failure(host_name: impl Into<String>, detail: impl Into<String>) -> Self {
        NodError::evaluation(format!(
            "Nix build failed for host {}: {}",
            host_name.into(),
            detail.into()
        ))
    }

    /// Raised when local `sudo switch-to-configuration` fails.
    pub fn local_activate(detail: impl Into<String>) -> Self {
        NodError::deployment(format!(
            "failed to activate local NixOS configuration: {}",
            detail.into()
        ))
    }

    /// Raised when the store-copy step over SSH fails.
    pub fn store_transfer(detail: impl Into<String>) -> Self {
        NodError::deployment(format!("Nix store copy over SSH failed: {}", detail.into()))
    }

    /// Raised when the remote switch activation over SSH fails.
    pub fn remote_activate(detail: impl Into<String>) -> Self {
        NodError::deployment(format!(
            "failed to execute remote activation over SSH: {}",
            detail.into()
        ))
    }

    /// Raised when a rollback step fails.
    pub fn rollback_failure(detail: impl Into<String>) -> Self {
        NodError::deployment(format!("rollback failed: {}", detail.into()))
    }

    /// Raised when a requested target host cannot be found.
    pub fn not_found(host: impl Into<String>) -> Self {
        NodError::config(format!(
            "Target host '{}' not found in flake nixosConfigurations.",
            host.into()
        ))
    }

    /// Raised when configuration (CLI / TOML / flake metadata) is malformed.
    pub fn config_parse(detail: impl Into<String>) -> Self {
        NodError::config(format!("configuration parse error: {}", detail.into()))
    }

    /// Raised when `AppContext` is asked for a service that has no binding.
    pub fn missing_binding(service: impl Into<String>) -> Self {
        NodError::config(format!(
            "no binding registered for service '{}'",
            service.into()
        ))
    }

    /// Raised when a host fails a reachability / verification probe.
    pub fn unreachable(detail: impl Into<String>) -> Self {
        NodError::healthcheck(format!("host failed health probe: {}", detail.into()))
    }

    /// Raised when an internal invariant is violated.
    pub fn invariant(detail: impl Into<String>) -> Self {
        NodError::internal(format!("internal invariant violated: {}", detail.into()))
    }

    /// Category constructor: evaluation failures.
    pub fn evaluation(detail: impl Into<String>) -> Self {
        NodError::Evaluation {
            detail: detail.into(),
        }
    }

    /// Category constructor: deployment failures.
    pub fn deployment(detail: impl Into<String>) -> Self {
        NodError::Deployment {
            detail: detail.into(),
        }
    }

    /// Category constructor: configuration failures.
    pub fn config(detail: impl Into<String>) -> Self {
        NodError::Config {
            detail: detail.into(),
        }
    }

    /// Category constructor: health check failures.
    pub fn healthcheck(detail: impl Into<String>) -> Self {
        NodError::HealthCheck {
            detail: detail.into(),
        }
    }

    /// Category constructor: internal failures.
    pub fn internal(detail: impl Into<String>) -> Self {
        NodError::Internal {
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_variants_classify_failure_origin() {
        let eval = NodError::discovery_failure("boom");
        assert!(matches!(eval, NodError::Evaluation { .. }));
        assert_eq!(
            eval,
            NodError::evaluation("Nix host discovery evaluation failed: boom")
        );

        let deploy = NodError::local_activate("sudo missing");
        assert!(matches!(deploy, NodError::Deployment { .. }));

        let config = NodError::not_found("atlas");
        assert!(matches!(config, NodError::Config { .. }));

        let health = NodError::unreachable("atlas");
        assert!(matches!(health, NodError::HealthCheck { .. }));

        let internal = NodError::invariant("unreachable state");
        assert!(matches!(internal, NodError::Internal { .. }));
    }

    #[test]
    fn messages_include_underlying_cause() {
        assert!(NodError::build_failure("jello", "eval failed")
            .to_string()
            .contains("jello"));
        assert!(NodError::build_failure("jello", "eval failed")
            .to_string()
            .contains("eval failed"));
        assert!(NodError::missing_binding("ConfigStorePort")
            .to_string()
            .contains("ConfigStorePort"));
        assert!(NodError::not_found("atlas").to_string().contains("atlas"));
    }

    #[test]
    fn errors_are_debug_printable_and_comparable() {
        let a = NodError::config_parse("bad toml");
        let b = a.clone();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        assert_eq!(a, b);
    }

    #[test]
    fn rollback_failure_is_a_deployment_error() {
        let err = NodError::rollback_failure("store transfer aborted");
        assert!(matches!(err, NodError::Deployment { .. }));
        assert!(err.to_string().contains("rollback"));
        assert!(err.to_string().contains("store transfer aborted"));
    }
}
