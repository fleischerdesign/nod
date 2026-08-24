# ADR-010: Lifecycle Command Execution Consolidation

- **Status:** Accepted
- **Date:** nod v2.1 hardening
- **Deciders:** nod maintainers
- **Supersedes:** n/a (tightens presentation layer structure from ADR-005 and ADR-008)

## Context

The deployment lifecycle commands (`switch`, `test`, `boot`) share an identical deployment orchestration sequence in the presentation layer:
1. Validating concurrency boundaries (`--concurrency >= 1`),
2. Parsing and resolving rollout strategies (`RolloutStrategy::parse`),
3. Invoking degraded host discovery on the evaluator (`discover_hosts_degraded`),
4. Resolving and gating target hosts with default local scoping (`resolve_targets(..., DefaultScope::Local)`),
5. Unmatched-set verification (`TargetSelection::unmatched`),
6. Materializing merged configuration overrides (`store.apply_to(host)`),
7. Dispatching the execution through `DeployFleetUseCase::execute`,
8. Conditionally formatting and rendering the summary output (`render_summary`).

Previously, this entire 60-line pipeline was duplicated verbatim across `src/commands/switch.rs`, `src/commands/test.rs`, and `src/commands/boot.rs`.

## Decision

Introduce a shared presentation execution helper `src/commands/lifecycle.rs::execute_lifecycle` that encapsulates this canonical lifecycle sequence.

Specific command modules (`switch.rs`, `test.rs`, `boot.rs`) remain the distinct CLI entry points for Clap dispatch, but their implementations delegate directly to `execute_lifecycle` after mapping CLI-specific arguments into `LifecycleParams` and the appropriate `DeploymentAction`.

## Consequences

- Eliminates ~180 lines of duplicate orchestration code in the presentation layer.
- Guarantees uniform error messages, target resolution semantics, and configuration application across all lifecycle commands.
- Simplifies future additions of deployment hooks or telemetry across all lifecycle commands to a single call site.
