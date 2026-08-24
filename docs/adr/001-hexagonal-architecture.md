# ADR-001: Hexagonal (Ports & Adapters) / Clean Architecture

- **Status:** Accepted
- **Date:** Foundation (nod v2.0.0 transformation)
- **Deciders:** nod maintainers

## Context

`nod` started as a compact command-line engine: `main.rs` parses a `clap` CLI, dispatches to per-command functions, and each command constructs its infrastructure types directly (`SshCliDeployer::new()`, `NixCliEvaluator::new()`). Domain logic, application control flow, and mechanical tool calls are interleaved:

- `main.rs` wires commands by hand and ties to concrete infrastructure.
- `switch.rs` / `status.rs` / `diff.rs` each construct `NixCliEvaluator` + `SshCliDeployer` directly.
- The single `SshCliDeployer` branches on `host.is_local`, so SSH and local deployment live in one adapter.
- Error handling is `anyhow::Result` everywhere, so no failure type survives a layer boundary intact (moved to ADR-002).

This works for a single-operator tool, but it cannot support what v2 needs: testable domain logic, a deployment state machine, config tiers, and fleet-scale concurrency (ADRs 003–005). The engine must be able to run the same policy logic against a fake/in-memory "deployer" in tests and against real SSH in production, without rewiring control flow.

## Decision

Adopt **Hexagonal Architecture (Ports & Adapters) with a Clean-Architecture layer ordering**:

1. **Domain Core** — pure, dependency-free Rust: entities and value objects (`HostEntity`, `SshProfile`, ...) and the **ports** (interfaces) that the rest of the world must satisfy:
   - `EvaluatorPort` — `discover_hosts`, `build_toplevel`.
   - `DeployerPort` — `check_reachability`, `deploy_and_activate`, `rollback`.
   - `ConfigSource` — configuration resolution for a host.
   - `HealthCheckPort` — post-activation verification.
2. **Application** — use cases (one per command) plus an `AppContext` dependency-injection container. The Application orchestrates *policy*: which hosts, in which order, with what concurrency/error-recovery, and how to drive the deployment state machine. It depends on Domain Core **only through ports**.
3. **Infrastructure** — concrete adapters implementing the ports:
   - `NixCliEvaluatorAdapter` (Nix CLI),
   - `SshDeployerAdapter` (SSH) and `LocalDeployerAdapter` (experimental local sudo switch),
   - `ConfigAdapter` (`.nod.toml` → flake metadata → defaults),
   - `StoreAdapter` (closure storage),
   - `PingHealthProbe` / `SshHealthProbe` health adapters.
4. **Presentation** — the CLI (`clap`) and the TUI (`ratatui` / dashboard). It parses user intent, formats results, and builds/seeds the `AppContext`. Concrete adapter wiring is centralized in the single composition root `src/commands/wiring.rs::production` (ADR-008), which `main.rs` calls for every command arm; apart from that root, Presentation never imports Infrastructure or Domain internals directly.

The **dependency rule is absolute**: source dependencies point inward toward the Domain Core. Domain imports nothing from Application/Infrastructure/Presentation. Application depends on Domain ports, not adapter types. Infrastructure and Presentation are free to depend on Domain and Application through the designated seams.

## Consequences

### Positive

- **Testability:** use cases run against in-memory/mock adapter implementations; no external Nix, SSH, or real clock required.
- **Adapters are swappable:** local vs SSH deployment is a resolution choice, not an `if is_local` branch in a command.
- **Single source of policy:** deployment lifecycle, rollback, rollout strategy and concurrency live once in Application, shared by CLI and TUI.
- **Onboarding clarity:** each layer has a clear, minimal import rule that is easy to review and enforce.

### Negative / Trade-offs

- **More modules initially:** the honest split costs several files and indirection versus today's flat command files.
- **Abstraction ceiling:** ports must stay policy-shaped; overabstracting mechanical detail adds ceremony without value.
- **Migration friction:** existing command (`switch`, `status`, `diff`, `check`) currently hold concrete types and must be rewired to `AppContext` resolution in lockstep with ADR-002.

## Options Considered

- **Monolith (current):** rejects; grows or testability gap open as v2 features land.
- **MVC-style (thin controllers over domain services):** rejected; does not give a clean port boundary for swapping the SSH/local/config/health mechanics.

## Compliance

- New Domain types must have zero imports from Application/Infrastructure/Presentation.
- `AppContext` is the only injection point; command wiring reaches Infrastructure only through resolved ports.
- Module rules are stated under [`../architecture/overview.md`](../architecture/overview.md). Compliance is confirmed by review and by the fact that the spec suite runs against mock adapters without touching the real Nix toolchain.