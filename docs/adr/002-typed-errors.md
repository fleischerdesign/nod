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
NodError                       // single enum
├── Evaluation  { detail }     // Nix eval / host discovery / closure build
├── Deployment  { detail }     // transfer / switch activation / rollback
├── Config      { detail }     // CLI, `.nod.toml`, flake metadata, defaults
├── HealthCheck { detail }     // reachability & post-activation verification
└── Internal    { detail }     // invariants, programming errors
```

Unlike a subtype hierarchy, the code models this as a **single enum** whose variants each carry one structured field, `detail: String`, holding the operator-facing message. Category constructors (`NodError::evaluation`, `NodError::deployment`, `NodError::config`, `NodError::health_check`, `NodError::internal`) and semantic constructors (`NodError::discovery_failure`, `NodError::build_failure`, `NodError::local_activate`, `NodError::not_found`, `NodError::unreachable`, ...) build the right variant while keeping the message friendly. Callers that branch on class match the outer variant; callers that only surface a message read `detail`.

`AppContext` / resolved ports declare `Result<()>` where `AppResult = Result<T, NodError>`, and adapters map their underlying tool failure into the matching variant via those constructors.

## Error ↔ Feature Mapping (concrete)

| Current call site (v2.0.0 tree)        | Mapped typed error                       |
|----------------------------------------|-----------------------------------------|
| “Target host … not found” (`switch.rs`) | `NodError::not_found(host)` (`Config` variant) |
| “Nix host discovery evaluation failed:” (`cli_evaluator.rs`) | `NodError::discovery_failure(stdout/stderr)` |
| “Failed to parse Nix host discovery JSON” | `NodError::parse_failure` |
| “Nix build failed for host …” | `NodError::build_failure(host, stderr)` |
| “Failed to activate local NixOS configuration” | `NodError::local_activate` |
| “Nix store copy over SSH failed” | `NodError::store_transfer` |
| “Remote activation failed / Failed to execute remote activation over SSH” | `NodError::remote_activate` |
| reachability probe failure (“ping”) | `NodError::unreachable` |
| malformed flake schema / config parse | `NodError::config_parse` |

## Consequences

### Positive

- **Classifier boundaries:** control flow can match on error `as` — the state machine and fleet controller inspect `NodError::Deployment` vs `NodError::HealthCheck` to pick recovery.
- **Recovery decisions:** rollback and `ContinueOnError` carry the right class into each handler.
- **Testability:** mock adapters can be made to return specific typed failures, exercised with assertions on domain logic.
- **Operator-facing messages** remain friendly via the `detail` field (and the `Display` impl) while the outer variant keeps the machine-readable class.

### Negative / Trade-offs

- Migration effort: every `anyhow::Result` return site is re-typed; `anyhow` import is gradually removed from the engine layers.
- More verbose declaration in each typed error variant (a judged, priced-in cost).

## Notes / Migration

- `anyhow` remains allowed only for the thin CLI entry. Application and below must return `NodError`.
- The migration is incremental: each command rewired to `AppContext` resolution (ADR-001) drops its own `anyhow` imports as it converts.
- `NodError` types live in `domain/errors` and are referenced by every Domain port signature.

## Compliance

Run `grep -rn 'anyhow' src/domain src/application` and expect **no matches**. Infrastructures/ adapters may, when mapping an external tool failure into a typed error, temporarily, but any non-boundary literal `anyhow!` is a review defect. Related: ADR-001 (ports), ADR-003 (state machine), ADR-005 (fleet error-recovery). The Gherkin `foundation.spec.md` nails down the observable behavior of error propagation.