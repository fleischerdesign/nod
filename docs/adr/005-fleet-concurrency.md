# ADR-005: Fleet Concurrency

- **Status:** Accepted (target-state for fleet rollout)
- **Date:** nod v2.0.0 transformation
- **Deciders:** nod maintainers

## Context

Today `switch --all` iterates hosts sequentially:

```rust
for host in targets {
    ...evaluator.build_toplevel(...).await?;
    ...deployer.deploy_and_activate(...).await?;
}
```

Serial execution is order-safe but slow for a fleet, and the current loop **cannot** express rollout safety (do not blast all machines at once for a breaking topological change) or a containment policy (keep going or abort when one host fails).

For ADR-003 each host is an event-driven per-host state machine. We need to run many of those concurrently while respecting:

- a **concurrency budget** (never exceed `N` in-flight deployments),
- **rollout strategy**: *Canary*, *Batch*, or *All*,
- **error-recovery**: `FailFast` (abort the whole run on the first failure) or `ContinueOnError` (mark the failed host degraded and press on).

## Decision

Drive fleet deployments with **Tokio Semaphore** gating `n` concurrent in-flight hosts, each host running its own ADR-003 state machine inside an async task. The Application builds a rollout plan (ordered host list + strategy + concurrency + recovery mode) and streams hosts through the semaphore.

```rust
// Application layer (use `tokio::sync::Semaphore`, max permits = concurrency)
sem := Semaphore::new(appctx.config.concurrency_default())   // set lower than pipeline capacity
tasks := vec![]
for host in plan.order() {
    tasks.push(
        appctx.SpawnFlight(
            executor.execute! { self.deployHost(host) },      // drives one ADR-003
            semaphore(sem),
        ),
    )
}
tasks converge (gather) and a per-host result is aggregated into the run summary.
```

### Rollout strategies

| Strategy  | Order                                            | Risk posture        |
|-----------|--------------------------------------------------|---------------------|
| **All**   | All hosts in parallel (bounded by `sem`).        | Fastest; whole fleet is a single failure blast radius. |
| **Batch** | Deploy in fixed-size waves (`--batch-size`); join each wave, check policy, then start the next. | Controlled; per-wave abort on `FailFast`. |
| **Canary**| Deploy a small "warm-up" host (or slice) first, observe its `Verifying` result, then continue with the batch/remaining fleet. | Safest; catches a bad closure on one machine before wide blast. |

Default strategy is `Batch` (conservative); tactic and scheduling order (`canary` slice, batch size, absolute concurrency) come from the config tier (ADR-004: CLI flag → TOML → flake → defaults).

### Error-recovery modes

- **FailFast**: if any host transitions into `ROLLBACK`/failure, the rollout cancels: no new host is scheduled; in-flight tasks finish their current step then leave; and the run returns a `FleetAborted` summary listing the failing host.
- **ContinueOnError** — a failed host is recorded (marked `degraded`/`ROLLBACK`), the semaphore slot frees, and the next host starts. The run completes with a partial-failure summary.

The recovery policy is decided **per host outcome**: it is applied by the rollout controller after each per-host task completion (check outcome → `FailFast`?).

## Consequences

### Positive

- **Faster fleet `switch --all`;** bounded memory and process pressure via the semaphore.
- **Safer fleet ops:** canary/batch reshape default blast radius; `FailFast` hard-aborts, `ContinueOnError` isolates.
- **Clean diagnostics:** each host's ADR-003 `DeploymentState` plus a per-host result type feed the run summary and TUI matrix.

### Negative / Trade-offs

- **A new, genuinely concurrent path**: partial-failure and cancel semantics must be tuned.
- **Saturation control** (batch gap, canary wait) adds latency vs vanilla parallel `All`.
- Adds config surface (strategy + error mode + batch size) in tier 1 (CLI) and tier 2/TOML of ADR-004.

## Compliance

- Host scheduling **and** per-host sequencing must always follow the active strategy; a host is never started before its turn under `Batch`/`Canary`.
- `FailFast` runs must not schedule additional hosts after the first failure (in-flight hosts already within the current stage may still finish it).
- The Gherkin spec `../spec/foundation.spec.md` pins the observable concurrency & recovery scenarios.

## Related

- ADR-001 (AppContext), ADR-003 (per-host state machine), ADR-004 (config tiers); `../spec/foundation.spec.md`.