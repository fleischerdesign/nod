# ADR-003: Deployment State Machine

- **Status:** Accepted (target-state for `nod v2`)
- **Date:** nod v2.0.0 transformation
- **Decisions:** adopt a single, explicit lifecycle for every host's deployment.

## Context

Today `switch.rs` drives deployment with inline sequential logic:

```rust
for host in targets {
    let closure = evaluator.build_toplevel(flake_path, &host.name, verbose).await?;
    deployer.deploy_and_activate(&host, &closure, verbose).await?;
}
```

There is **no explicit lifecycle state**; a step either succeeds or the whole propagation bails via `?`. That works for a one-shot local switch but not for:

- **Failures mid-activation** — a half-applied NixOS config leaves a machine in an unknown state; today there is no defined *rollback path*.
- **Verification** — activation says “switch done,” but nothing checks the *running binary TRUE health*.
- **Fleet concurrency (ADR-005)** — parallel hosts must be in a distinct, observable state at any instant (which is still *building*, *transferring*, *switching*…) so rollout and reporting are coherent.
- **Auditability** — operators / TUI need to see each host’s live step.

## Decision

Model each host deployment as an **explicit state machine** owned by Application:

```text
                 ┌──────────────┐
                 │   Prepped    │
                 └──────┬───────┘
                        │ begin
                        ▼
                 ┌──────────────┐
                 │  Evaluating  │   ← discovery / closure eval
                 └──────┬───────┘
                        │                eval failure → ROLLBACK
                        ▼
                 ┌──────────────┐
                 │   Building   │   ← nix build toplevel
                 └──────┬───────┘
                        │                build failure → ROLLBACK
                        ▼
                 ┌──────────────┐
                 │ Transferring │   ← copy closure (store / SSH)
                 └──────┬───────┘
                        │                transfer failure → ROLLBACK
                        ▼
                 ┌──────────────┐
                 │   Switching  │   ← activate switch-to-configuration
                 └──────┬───────┘
                        │                switch failure → ROLLBACK
                        ▼
                 ┌──────────────┐
                 │  Verifying   │   ← health / closure probe
                 └──────┬───────┘
                        │                verify failure → ROLLBACK
                        ▼
                 ┌──────────────┐
                 │  Completed   │   end state
                 └──────────────┘
```

- **Every transition is guarded**: a host may only advance to the *successor* state, and only when the prior step returned `Ok`.
- **On any failure at states Evaluating…Verifying**, the host drives `→ ROLLBACK` (reversion to the previously known-good generation profile). Rollback is itself a first-class step with an explicit outcome.
- `ROLLBACK` is handled as the terminal recovery state; a failed rollback yields `DeploymentRollbackError` (ADR-002) and the host is marked **degraded**, not `Completed`.
- A host is **Completed** only after a successful `Verifying`.

### API shape (Application layer)

```rust
enum DeploymentState {
    Prepped,
    Evaluating,
    Building,
    Transferring,
    Switching,
    Verifying,
    Completed,
    Rollback,   // entered on failure; terminal-recovery
}
```

The machine is stored per-host (fleet-wide in ADR-005) and advanced by the use case:

```rust
let st = AppContext.resolve::<DeploymentStateMachine>();
```

Application drives `Next()/Fail()`; each port call is sandwiched between the correct state transitions; observability (Presentation/TUI + telemetry) reads `state.named()` without ever mutating it.

## Consequences

### Positive

- **Predictable recovery:** every failure has a defined next step → choose *rollback* vs *continue* is explicit policy, not an accident.
- **Fleet coherence:** with per-host states, operational frames, and rollout/admission overlap with ADR-005.
- **Observability:** CLI `verbose`, TUI matrix, and tracing output the exact stage each host is in.
- **Verification as a guarantee:** "Completed" now means "activated *and* verified", not merely issued.

### Negative / Trade-offs

- More moving bookkeeping than today's flat loop (one recorder per host).
- Introduces an explicit `Rollback` operational cost: a fail must (usually) rebuild/re-verify the prior state rather than just printing an error.
- If verification is skipped (as in the current `nod switch` perf shortcut), the machine risks reporting "Completed" without an actual confirmed-good system — so the short-circuit is a conscious trade documented here.

## Compliance / Related

- The state graph in the Gherkin spec `../spec/foundation.spec.md` must match this document.
- ADR-002 supplies the typed transitions (rollback, deployment failure classification).
- ADR-005 composes these per-host machines under a rollout concurrency controller.