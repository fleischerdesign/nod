# nod — Architecture Roadmap

> **Status:** Active — tracks `nod` v2 capabilities against the ADRs under [`../adr`](../adr) and the specs under [`../spec`](../spec). Entries marked **✅ shipped** are fully implemented, tested, and shipped; the rest are planned capabilities.

## Overview & Purpose

`nod` is the **Universal Nix Orchestration & Deployment Engine v2**: it discovers
`nixosConfigurations`, evaluates them to store closures, transfers and switches
system configurations on local or remote machines (with or without bootloader
registration), manages Nix profiles & flake lockfiles, verifies health, and reports diagnostics.

Every host-operating entry inherits the **Unified Target Selector** (ADR-006):
`[TARGET/GLOB]`, `--tag`, `--role`, and `--all` behave identically to `switch`,
narrowing by **boolean AND (set-intersection)** and staying order-free.

The engine is built on robust architectural foundations:

- **Single composition root** — `wiring::production(...)` in
  `src/commands/wiring.rs`, called by `main.rs`, constructs the DI container with evaluator, deployers, config store, audit store, and generation store (ADR-008, ADR-014, ADR-015).
- **Effective connection profile** — the resolved `SshProfile` flows through the
  deploy port from the caller (ADR-007).
- **Centralized target resolution** — `resolve_targets` + `DefaultScope` in
  `application/selection.rs` is the single ADR-006 selection idiom used by every
  command (ADR-008).
- **Quality Gates** — strict CI quality gates (`cargo test` 100%, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `deadnix`, `statix`, `nixfmt`).

---

## Theme 1 — Lifecycle & Deployment

The core activate/switch lifecycle: compile, transfer, and activate per-generation.

| Command | Behavior | Status | Architectural fit |
|---|---|---|---|
| `nod switch [TARGET/GLOB] [--tag] [--role] [--all]` | Build, copy, and **activate** — write the new generation to the store and register it as the boot default via `switch-to-configuration`. | ✅ shipped | Application use case + transfer/switch ports + ADR-006 selector + ADR-003 state machine. |
| `nod test [TARGET/GLOB] [--tag] [--role] [--all]` | **Temporarily** activate without touching the bootloader (`switch-to-configuration test`). | ✅ shipped | App: shared deploy pipeline with test action flag (ADR-010). |
| `nod boot [TARGET/GLOB] [--tag] [--role] [--all]` | Register generation as bootloader default **without** immediate live service switch (`switch-to-configuration boot`). | ✅ shipped | App: shared deploy pipeline with boot action flag (ADR-010). |
| `nod build [TARGET/GLOB] [--tag] [--role] [--all] [--builder HOST] [--out-link L]` | Pure compilation of `system.build.toplevel` closures with optional remote builder delegation. | ✅ shipped | App: EvaluatorPort build path (ADR-006, ADR-010). |
| `nod plan [TARGET/GLOB] [--tag] [--role] [--all]` | Dry-run execution plan and closure diffing without performing changes. | ✅ shipped | Read-only App use case over discovery + store diff (ADR-012). |
| `nod rollback [TARGET]` | Revert to previous NixOS generation with automatic state verification. | ✅ shipped | RollbackUseCase + ADR-003 state machine + history store (ADR-013). |

---

## Theme 2 — Flake & Lockfile Management

Keep the input source of truth consistent, inspected, and updated before deployments.

| Command | Behavior | Status | Architectural fit |
|---|---|---|---|
| `nod update [INPUTS...]` | Selective or fleet-wide lockfile update with before/after revision and date delta summaries. | ✅ shipped | `UpdateFlakeUseCase` + `FlakePort` / `NixCliFlakeStore` (ADR-014). |
| `nod inputs` | Inspect declared and locked flake input hierarchies, URLs, commits, and follows-mappings. | ✅ shipped | `ListInputsUseCase` + `FlakePort` (ADR-014). |
| `nod metadata` | Inspect flake root path, repository lockfile version, git revision, and input count. | ✅ shipped | `InspectMetadataUseCase` + `FlakePort` (ADR-014). |
| `nod check` | Strict quality-gate runner: `nixfmt` + `deadnix` + `statix`. | ✅ shipped | `CheckUseCase` (ADR-001). |

---

## Theme 3 — Generations & Store Management

Manage store profiles, garbage collection, pre-staging, and deduplication.

| Command | Behavior | Status | Architectural fit |
|---|---|---|---|
| `nod generations [TARGET/GLOB] [--tag] [--role] [--all]` | Inspect installed system profile generations, UTC creation dates, active status, and store paths. | ✅ shipped | `ListGenerationsUseCase` + `StorePort` (ADR-015). |
| `nod gc [TARGET/GLOB] [--tag] [--role] [--all] [--keep N] [--older-than X] [--dry-run]` | Remote & local garbage collection with retention rules and dry-run preview. | ✅ shipped | `CollectGarbageUseCase` + `StorePort` (ADR-015). |
| `nod copy [TARGET/GLOB] [--tag] [--role] [--all] [--to URI] [--from URI]` | Pre-stage system closures on remote targets without activating them. | ✅ shipped | `CopyClosureUseCase` + `StorePort` (ADR-015). |
| `nod store optimize [TARGET/GLOB] [--tag] [--role] [--all]` | Hardlink deduplication via `nix-store --optimise`. | Planned | Infra: store adapter. |

---

## Theme 4 — Day-2 Operations & Inspection

Interactive management, remote execution, expression evaluation, and fleet orchestration.

| Command | Behavior | Status | Architectural fit |
|---|---|---|---|
| `nod ssh [TARGET] [--sudo] [-- <CMD>]` | Open interactive SSH shell or run remote command using resolved credentials. | ✅ shipped | Presentation + App context with resolved SshProfile (ADR-007). |
| `nod exec [TARGET/GLOB] [--tag] [--role] [--all] [--sudo] [-- <CMD>]` | Parallel multi-host remote command runner with concurrency & fail-fast bounding. | ✅ shipped | `ExecFleetUseCase` + SSH transport (ADR-005, ADR-007). |
| `nod eval <EXPR> [TARGET/GLOB] [--tag] [--role] [--all] [--raw] [--json]` | Evaluate Nix configuration attributes directly in host context. | ✅ shipped | `EvalFleetUseCase` + `EvaluatorPort` (ADR-016). |
| `nod repl [TARGET]` | Interactive `nix repl` pre-seeded with `{ flake, host, config, options, pkgs }`. | ✅ shipped | `nod::commands::repl` + Nix CLI (ADR-016). |
| `nod info [TARGET/GLOB] [--tag] [--role] [--all] [--json]` | Comprehensive host dashboard: kernel, uptime, generations, closures, systemd health. | ✅ shipped | `InspectInfoUseCase` + telemetry aggregation (ADR-016). |
| `nod reboot [TARGET/GLOB] [--tag] [--role] [--all] [--strategy S] [--wait]` | Rolling orchestrated host reboots with automatic reachability & health verification. | ✅ shipped | `RebootFleetUseCase` + `DeployerPort::reboot` (ADR-017). |
| `nod dashboard` | Interactive Ratatui TUI with live fleet matrix, detail pane, and audit log stream. | ✅ shipped | `ui::app` event loop with live action dispatch (ADR-009). |
| `nod audit [TARGET] [--limit N] [--json]` | Persistent append-only deployment and rollback audit trail. | ✅ shipped | `AuditLogUseCase` + `JsonAuditStore` (ADR-013). |
| `nod diff [TARGET/GLOB] [--tag] [--role] [--all]` | Package version and systemd unit comparison between live system and flake. | ✅ shipped | `DiffUseCase` + closure extraction (ADR-012). |
| `nod drift [TARGET/GLOB] [--tag] [--role] [--all]` | Detect configuration drift between live closures and repository flake. | ✅ shipped | `DetectDriftUseCase` (ADR-001). |

---

## Theme 5 — Secrets & Security

| Command | Behavior | Status | Architectural fit |
|---|---|---|---|
| `nod secret check [TARGET/GLOB] [--tag] [--role] [--all]` | Pre-flight secret decryptability verification (sops/age). | Planned | Infra: sops/age store adapter. |
| `nod secret rekey [TARGET/GLOB] [--tag] [--role] [--all]` | Fleet-wide age recipient rotation and re-encryption. | Planned | Infra: sops/age store adapter + rollout plan. |

---

## Theme 6 — Day-0 Provisioning & GitOps

| Feature | Behavior | Status | Architectural fit |
|---|---|---|---|
| `nod bootstrap <TARGET> --ip <IP> [--disko]` | Bare-metal installer from live ISO using `nixos-anywhere` and `disko`. | Planned | `ProvisioningPort` + nixos-anywhere adapter. |
| `nod init [--template ...]` | Scaffold a new flake repository with `nod.nixosModules.default`. | Planned | App template engine. |
| `nod watch [TARGET]` | Live auto-preview: rebuild and diff on file changes. | Planned | App file watcher + plan use case. |
| `nod sync` / `nod daemon` | Pull-based GitOps background reconciler. | Planned | Daemon service + rollout controller. |