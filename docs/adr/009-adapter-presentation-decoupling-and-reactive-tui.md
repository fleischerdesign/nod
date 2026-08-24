# ADR-009: Adapter Presentation Decoupling & Reactive TUI Execution

- **Status:** Accepted
- **Date:** nod v2.1 hardening
- **Deciders:** nod maintainers
- **Supersedes:** n/a (extends ADR-001 Hexagonal Architecture and ADR-008 Composition Root)

## Context

Under ADR-001 (Hexagonal Architecture), Domain and Infrastructure layers must not depend on Presentation. However, two architectural violations were identified in practice:

1. **Direct Terminal I/O in Adapters (`println!`):**
   `LocalDeployer`, `SshCliDeployer`, and `QualityGate` directly invoked `println!` to print status lines ("Activating local configuration...", "Copying closure to ..."). When invoked from non-CLI presentation contexts (such as the Ratatui TUI dashboard or background daemon tasks), this directly polluted stdout, corrupted TUI rendering frames, and violated the boundary where Presentation owns formatting.

2. **Blocking TUI Event Loop:**
   In `src/ui/mod.rs`, user action intents (`s` for switch, `r` for rollback, `d` for diff) invoked `DeployFleetUseCase::execute(...).await` synchronously inside the TUI event loop frame handler (`drive_events`). Because deployments involve compilation, store transfer, and system activation taking seconds or minutes, the TUI frame rendering froze entirely: animations stopped, key inputs (like `q` for quit) were blocked, and terminal redraws ceased until the operation completed.

## Decision

### 1. Zero Direct Terminal Output in Infrastructure Adapters
All `println!` calls inside `LocalDeployer` and `SshCliDeployer` are removed. Infrastructure adapters communicate operational context solely via:
- Return values (`Result<T, NodError>`),
- Structured diagnostics using the standard `tracing` ecosystem (`tracing::info!`, `tracing::debug!`).

Console output and progress indications remain the sole responsibility of the Presentation layer (`src/commands/*` for CLI and `src/ui/*` for TUI).

### 2. Asynchronous, Non-Blocking TUI Background Execution
Action dispatches in `src/ui/mod.rs` and `src/ui/app.rs` are executed as non-blocking asynchronous tasks (`tokio::spawn`). 
- The TUI maintains an asynchronous message channel (`tokio::sync::mpsc`) to stream status and log updates back to the event loop.
- The TUI rendering loop continues to draw at its regular tick interval (rendering active operation spinners and log entries), while remaining responsive to user navigation and quit commands.

## Consequences

- Infrastructure adapters are completely decoupled from stdout and can safely be used in CLI, TUI, or library environments without side effects.
- The TUI remains fluid, responsive, and animated during long-running builds, deployments, and rollbacks.
- Observability is unified under standard `tracing` subscribers.
