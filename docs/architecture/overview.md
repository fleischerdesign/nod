# nod — Architecture Overview

> **Status:** Foundation (target-state) — this document describes the Hexagonal / Clean Architecture layout that `nod` v2 is being transformed toward. Some layers are already present; others (Application, typed errors, configuration hierarchy, state machine, fleet concurrency) are landing incrementally. See the ADRs under [`../adr`](../adr) for individual decisions.

## Purpose

`nod` is the **Nix Orchestration & Deployment Engine**. Given a Nix flake, it discovers `nixosConfigurations` host entries, evaluates them to store closures, transfers and switches configurations on local or remote machines, verifies the result, and reports status. The architecture keeps the *deployment policy logic* independent from the *mechanical details* of running Nix commands, SSH, or parsing config, so the engine is testable in isolation and adapters can be swapped without touching domain logic.

## Layer Map

```
┌─────────────────────────────────────────────────────────────────────┐
│                         PRESENTATION                                 │
│                                                                     │
│   CLI (clap)          TUI (ratatui)      Telemetry (tracing)        │
└──────────────────────────────┬──────────────────────────────────────┘
                               │  calls
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         APPLICATION                                  │
│                                                                     │
│   Use Cases (SwitchUseCase, StatusUseCase, CheckUseCase,             │
│              DiffUseCase, RollbackUseCase)                           │
│   AppContext   ← dependency-injection container resolving           │
│                  AppServices & Ports                                 │
│   DeploymentStateMachine (see ADR-003)                               │
│   FleetRollout / RolloutStrategy (see ADR-005)                       │
└──────────────────┬───────────────────────┬──────────────────────────┘
                   │ (port)               │ (port)
                   ▼                       ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         DOMAIN CORE                                  │
│                                                                     │
│   Entities & Value Objects                                          │
│     Host, SshProfile, DeploymentState, FlakeReference...            │
│   Ports (interfaces)                                                │
│     EvaluatorPort,   DeployerPort, ConfigSource,                   │
│     HealthCheckPort, MachineClock                                  │
│                                                                     │
│        ↻ NO dependencies: pure Rust types + thiserror domain errors │
└──────────────────┬───────────────────────┬──────────────────────────┘
                   │ (implemented by)      │ (implemented by)
               ▼                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                       INFRASTRUCTURE                                 │
│                                                                     │
│   NixCliEvaluatorAdapter     (Nix CLI)                               │
│   SshDeployerAdapter         (SSH transport)                         │
│   LocalDeployerAdapter       (local sudo switch-to-configuration)    │
│   TomlConfigSource + FlakeConfigSource  (config)                     │
│   JsonStorage / StoreAdapter (storage/closure store)                 │
│   PingHealthProbe, SshHealthProbe      (health checks)               │
└─────────────────────────────────────────────────────────────────────┘
```

## Layers

### Presentation

The outermost shell. Translates user intent into application-layer use cases and formats the results back.

| Sub-system  | Responsibility                                              |
|-------------|-------------------------------------------------------------|
| **CLI**     | Parses `clap` command structure (`switch`, `check`, `status`, `diff`, `rollback`, `dashboard`), global `-v/--verbose`, `-q/--quiet`, and constructs the `AppContext`. |
| **TUI**     | Ratatui dashboard/widgets for fleet management; reserved for the full-screen interactive view. |
| **Telemetry**| OpenTelemetry tracing + `EnvFilter`; instrumentation observed at layer boundaries. |

The Presentation layer **must not** import Infrastructure adapters or Domain internals directly — it asks the Application layer for a resolved `AppContext`.

### Domain Core (pure Rust)

The innermost, dependency-free layer. It contains:

- **Entities** — long-lived stateful objects: `HostEntity`, `HostRole`.
- **Value objects** — immutable, structural: `SshProfile`, `ActiveClosure`, `ConfigError`/typed-error types, deployment transition guards.
- **Ports** — the interfaces the outside world must satisfy so Domain/Application never depend on an external tool:
  - `EvaluatorPort` — discover hosts, build a toplevel closure.
  - `DeployerPort` — reachability probe, deploy-and-activate, rollback. Transport methods take the resolved `&SshProfile` alongside the host (ADR-007).
  - `ConfigSource` — supply resolved configuration for a host.
  - `HealthCheckPort` — verify a live system after activation.

Domain Core has **zero imports from Infrastructure, Application, or Presentation.** Its only external dependency is `serde`/`thiserror`-style type definitions — and those are treated as part of the type system, not a policy leak.

### Application (use cases + AppContext)

The Application layer is where **orchestration lives**: it owns the *policy* (which hosts, in which order, with which concurrency and error-recovery, how to sequence the deployment state machine) but *not* the mechanics.

- **AppContext** — a dependency-injection container. `main` (Presentation) is the **single composition root**: it builds the context through one factory, `wiring::production(flake_path, cli_overrides)` in `src/commands/wiring.rs` (see ADR-008), which constructs the evaluator, both deployers, and the `TomlConfigStore` (bound as `ConfigStorePort`). Commands no longer construct `AppContext` or the store themselves; they accept a resolved context and bind optional services (e.g. `audit`'s `AuditStorePort`) at an explicit call site.
- **Effective flake root** — every command resolves the flake path for the run through the single `effective_flake` cascade in `src/infrastructure/config/toml_config.rs`: explicit CLI `--flake` > `[defaults].flake` of the nearest `.nod.toml` (walked up from the invocation cwd; relative values resolve against the config file's directory) > the working directory. `drift`/`audit`/`ssh` expose `--flake` like all other flake-scoped commands. No `flake` option exists in `config.nod` — the flake location is a Nix-side input, so the NixOS module deliberately stays out of that chicken-egg.
- **Effective profile flow** — the resolved `SshProfile` is an explicit argument to the deploy port (ADR-007). The caller obtains it via `ConfigStorePort::resolve(host)` / `AppContext::resolved_profile(host)`; the transport adapters are dumb transports that consume only the passed profile instead of re-deriving a primitive one. `ssh`/`exec` now bind a config store, so resolved connection settings are honoured on those paths.
- **Centralized target resolution** — `resolve_targets` + `DefaultScope` in `application/selection.rs` implement the shared ADR-006 target-selection idiom at one call site per command (ADR-008): deploy/exec default to `DefaultScope::Local`, `status` to `DefaultScope::All`, and `select_exact_one` stays the single-host gate for `rollback`/`ssh`.
- **Use cases** — one per command: `switch`, `check`, `status`, `diff`, `rollback`, `dashboard`. Each expresses *"what should happen"* and delegates mechanical steps to the resolved port implementations.

Application depends on Domain Core *only through ports*, and is independent of any concrete adapter.

### Infrastructure (adapters)

Concrete implementations of the Domain ports. Each wraps a particular external mechanism behind a port so the rest of the engine stays tool-agnostic:

| Port          | Adapter(s)                        | Backing            |
|---------------|-----------------------------------|--------------------|
| Evaluator     | `NixCliEvaluatorAdapter`           | `nix eval`, `nix build` CLI |
| Deployer      | `SshDeployerAdapter`               | SSH (`ssh`, `nix copy`) |
| Deployer      | `LocalDeployerAdapter`            | `sudo switch-to-configuration` |
| Config        | `ConfigAdapter` (see ADR-004)      | `.nod.toml` → flake metadata → defaults |
| Storage       | `StoreAdapter`                     | local closure store / store paths |
| HealthCheck   | `PingHealthProbe`, `SshHealthProbe`| reachability + service probes |

The `SshDeployerAdapter` vs `LocalDeployerAdapter` split removes the `is_local` branching currently baked into a single deployer (see `src/infrastructure/tokio_ssh.rs` in the current tree) — each host resolves through its own deployer based on target identity.

## Data Flow

A canonical `switch` flow over the layers, tracing one host from CLI to verified:

```
Presentation (CLI)
   │  parse(clap) → build AppContext
   ▼
Application (SwitchUseCase)
   │  AppContext.resolve(Evaluator)          → Domain Validator (port)
   │  hosts = evaluator.discover(flake)    → Domain HostEntity list
   │  state advance: Prepped → Evaluating
   │  closure = evaluator.buildToplevel(flake, host)
   │  state Evaluating → Building
   │  DeployerPort (Local or Ssh)
   │     deploy_and_activate(host, closure)
   │  state Building → Transferring → Switching
   │  verify(host) → HealthCheckPort
   │  state Verifying → Completed   (else → Rollback)
   │  trace: emit completed/rolled-back metric
   ▼
Infrastructure (adapter) → external Nix/SSH/local OS
   ▼
Development/diagnostic output via Presentation + structured telemetry
```

### Dependency Direction & The Rule

- Dependencies point **inward**: Presentation → Application → Domain. Conversation never goes outward from Domain.
- Infrastructure **implements** the outward-facing ports defined by Domain; it is *depended on* by the Application only through those ports.
- The rule: **"no source dependency may cross a layer boundary toward the center."** Enforcement is manual today (module discipline) and is reinforced by the typed error seam (every adapter returns port-shaped `NodError`).

## Module Layout (target)

```
src/
├── domain/               # Domain core — pure
│   ├── entities/         # Host, HostType
│   ├── values/           # SshHost, ActiveClosure, DeploymentConfig
│   ├── ports/            # HostPort, Deployer, ConfigSource, HealthCheck
│   └── errors/           # NodError (variant hierarchy)
├── application/
│   ├── context.rs        # AppContext (DI container)
│   └── use_cases/        # switch, check, status, diff, rollback
├── infrastructure/
│   ├── adapters/
│   │   ├── cli_evaluator.rs
│   │   ├── ssh_deployer.rs
│   │   ├── local_deployer.rs
│   │   ├── config.rs
│   │   └── health.rs
│   └── (transport, storage)
├── presentation/
│   ├── cli.rs
│   └── tui.rs
└── main.rs
```

> During the transformation the flat `src/domain/`, `src/config/`, `src/infrastructure/`, `src/commands/`, `src/ui/` tree is migrated to this layout incrementally; each ADR lists the concrete next step.

## Related Documents

- ADR-001 — Hexagonal Architecture (`../adr/001-hexagonal-architecture.md`)
- ADR-002 — Typed Error hierarchy (`../adr/002-typed-errors.md`)
- ADR-003 — Deployment State Machine (`../adr/003-deployment-state-machine.md`)
- ADR-004 — Configuration Hierarchy (`../adr/004-configuration-hierarchy.md`)
- ADR-005 — Fleet Concurrency (`../adr/005-fleet-concurrency.md`)
- ADR-006 — Unified Target Selection (`../adr/006-unified-target-selection.md`)
- ADR-007 — Effective Connection Profile (`../adr/007-effective-connection-profile.md`)
- ADR-008 — Single Composition Root & Centralized Target Resolution (`../adr/008-composition-root.md`)
- Gherkin / foundation spec (`../spec/foundation.spec.md`)