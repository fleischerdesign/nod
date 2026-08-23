# ADR-002: Typed Domain Error Hierarchy (`NodError`)

- **Status:** Accepted (transition in progress)
- **Date:** nod v2.0.0 transformation
- **Deciders:** nod maintainers

## Context

Every failure path in `nod` today returns `anyhow::Result<T>` with ad-hoc message strings from `anyhow!("...")`:

```rust
return Err(anyhow!("Target host '{}' not found in flake nixosConfigurations.", target));
return Err(anyhow!("Nix build failed for host {}: {}", host_name, stderr));
```

Callers can only surface a message; they cannot **branch on the kind of failure**. Concretely:

- A command cannot distinguish *Host not found / Reachability down* from *Nix evaluation failed*.
- Rollback triggers in ADR-003 need to know the failure class (build vs transfer vs activation vs verification) to decide recovery.
- Fleet error-recovery in ADR-005 (`FailFast` vs `ContinueOnError`) must classify failures to apply the right strategy.
- `main.rs` propagates `anyhow::Result` to the top, so no error/code/boundary survives intact.

Opaque errors defeat the layered architecture: a typed error is the only seam that can cross the Domain → Application → Presentation boundary without strings.

## Decision

Introduce a **typed, `thiserror`-capable domain error hierarchy** rooted at `NodError`, replacing `anyhow::Result` at every Domain, Application, and adapter boundary.

```text
NodError
├── EvaluationError     // Nix eval / host discovery / closure build
├── DeploymentError     // transfer / switch activation / rollback
├── ConfigError         // CLI, `.nod.toml`, flake metadata, defaults
├── HealthCheckError    // reachability & post-activation verification
└── InternalError       // invariants, programming errors
```

Each node carries structured flags where they buy behavior:

- `NodEvaluationError` (SDK / discovery error)
- `NodDeploymentError`
- `NodConfigError`
- `NodHealthCheckError`
- an internal `NodInternalError` fallback.

`AppContext` / resolved ports declare `Result<()>` where `AppResult = Result<T, NodError>`, and adapters map their underlying tool failure into the matching variant.

## Error ↔ Feature Mapping (concrete)

| Current call site (v2.0.0 tree)        | Mapped typed error                       |
|----------------------------------------|-----------------------------------------|
| “Target host … not found” (`switch.rs`) | `NodConfigError.not_found(host)` for unknown target. |
| “Nix host discovery evaluation failed:” (`nix_evaluator.rs`) | `NodEvaluationError::discoveryFailures(stdout/stderr)` |
| “Failed to parse Nix host discovery JSON” | `NodEvaluationError::parseFailure` |
| “Nix build failed for host …” | `NodEvaluationError::buildFailure(host, stderr)` |
| “Failed to activate local NixOS configuration” | `NodDeploymentError::localActivate` |
| “Nix store copy over SSH failed” | `NodDeploymentError::storeTransfer` |
| “Remote activation failed / Failed to execute remote activation over SSH” | `NodDeploymentError::remoteActivate` |
| reachability probe failure (“ping”) | `NodHealthCheckError::unreachable` |
| malformed flake schema / config parse | `NodConfigError::parse` |

## Consequences

### Positive

- **Classifier boundaries:** control flow can match on error `as` — the state machine and fleet controller inspect `DeploymentError` vs `HealthCheckError` to pick recovery.
- **Recovery decisions:** rollback and `ContinueOnError` carry the right class into each handler.
- **Testability:** mock adapters can be made to return specific typed failures, exercised with assertions on domain logic.
- **Operator-facing messages** remain friendly via a trailing `message()` while structured variants keep the machine-readable class.

### Negative / Trade-offs

- Migration effort: every `anyhow::Result` return site is re-typed; `anyhow` import is gradually removed from the engine layers.
- More verbose declaration in each typed error variant (a judged, priced-in cost).

## Notes / Migration

- `anyhow` remains allowed only for the thin CLI entry. Application and below must return `NodError`.
- The migration is incremental: each command rewired to `AppContext` resolution (ADR-001) drops its own `anyhow` imports as it converts.
- `NodError` types live in `domain/errors` and are referenced by every Domain port signature.

## Compliance

Run `grep -rn 'anyhow' src/domain src/application` and expect **no matches**. Infrastructures/ adapters may, when mapping an external tool failure into a typed error, temporarily, but any non-boundary literal `anyhow!` is a review defect. Related: ADR-001 (ports), ADR-003 (state machine), ADR-005 (fleet error-recovery). The Gherkin `foundation.spec.md` nails down the observable behavior of error propagation.