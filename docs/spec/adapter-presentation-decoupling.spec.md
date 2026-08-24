# Spec: Adapter Presentation Decoupling & Reactive TUI

**Module:** `src/infrastructure/deployment/local_deployer.rs`, `src/infrastructure/deployment/ssh_cli_deployer.rs`, `src/ui/mod.rs`, `src/ui/app.rs`, `src/ui/views.rs`.
**References:** ADR-001 (Hexagonal Architecture), ADR-008 (Composition Root), ADR-009 (Adapter Decoupling & Reactive TUI).

## Problem

1. `LocalDeployer` and `SshCliDeployer` contained direct `println!` statements emitting status messages to stdout during deployment and rollback. This leaked formatting and I/O into the infrastructure layer, breaking encapsulation and corrupting full-screen TUI rendering.
2. In `src/ui/mod.rs`, `run_action` executed deployments inline on the main UI thread with `.await`, completely freezing frame rendering, spinner ticks, and keyboard responsiveness during builds and network transfers.

## Decision & Acceptance Criteria

### AC1 — Zero `println!` in Deployment Adapters
- Remove all `println!` calls from `LocalDeployer` and `SshCliDeployer`.
- Replace them with structured `tracing::info!` and `tracing::debug!` calls where logging is appropriate.
- Adapters communicate purely via return types `Result<(), NodError>`.

### AC2 — Non-Blocking TUI Background Worker
- In `src/ui/mod.rs`, user actions (`Switch`, `Rollback`, `Diff`) spawn a background asynchronous task (`tokio::spawn`).
- Background tasks send log messages and status updates to the TUI app state via an asynchronous channel or thread-safe shared event queue.
- The TUI rendering loop continues to draw frames, tick spinners, and accept keyboard events (`q` for exit, navigation, tab switching) without hanging while a deployment is running in the background.
- Concurrent operations on the same host or multiple conflicting deployments are guarded with an in-progress status flag.

## Verification
- `cargo test` — 100% pass.
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check` — clean formatting.
- `grep -rn "println\!" src/infrastructure/deployment/` returns 0 results.
