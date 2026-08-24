# Spec: Application & Port Cleanup (P4)

**Module:** `src/application/use_cases/deploy_fleet.rs`, `src/application/selection.rs`,
`src/application/pipeline/state_machine.rs`, `src/domain/ports/audit_store.rs`,
`src/domain/errors.rs`, `src/application/use_cases/generate_plan.rs`,
`src/domain/plan.rs`, `src/infrastructure/storage/json_audit_store.rs`, and callers.
**References:** ADR-003, ADR-005.
**Depends on:** P1-P3 in place.

## Problem (verified facts)

1. **Health verification is silently skipped (W9).** In `deploy_fleet.rs:345-357`,
   `if let Some(health) = ctx.health_checker_opt()` — when no checker is bound, the code
   still `tick(VerifyOk)` → `Completed` with `ok=true`. A missing checker silently counts
   as "verified". `HostOutcome` has no representation of "unverified". **However, no
   production command binds a `HealthCheckerPort`, so failing hard on a missing checker
   would break every deploy.** The honest minimal fix is to *signal* unverified rather
   than silently pass, not to fail.

2. **Dead code.** `TargetSelection::select_filtered` (selection.rs:237) has zero
   production callers (tests only). `DeploymentStateMachine::name()`/`can()`
   (state_machine.rs:110-119) have zero callers anywhere, marked `#[allow(dead_code)]`.

3. **ISP leak.** `AuditStorePort::record(&HostEntity, ...)` (audit_store.rs:14) uses only
   `host.name` in the impl (json_audit_store.rs:158). Couples the port to the rich entity.

4. **Error naming.** `NodError::HealthCheck` variant vs `NodError::healthcheck(...)`
   constructor — cosmetic inconsistency within an otherwise snake_case-consistent set.

5. **Plan duplication.** `generate_plan.rs::plan` and `deploy_fleet.rs::plan_for` both
   iterate hosts → `TargetPlan` → `DeploymentPlan`. The shared construction can be
   factored into `DeploymentPlan::from_hosts`.

6. **DeployFleet SRP / duplicated runner.** `run_wave` and `build_only` both hand-roll a
   semaphore + `JoinSet` bounded runner with cloned captures.

## Decision & Acceptance Criteria

### AC1 — Honest health signal (W9)
Add `pub health_verified: Option<bool>` to `HostOutcome`:
- `Some(true)` — checker present and passed.
- `Some(false)` — checker present and failed (host → `VerifyFail`, `ok=false`,
  `health_verified=Some(false)`).
- `None` — **no checker bound**; host still completes (`Completed`), but
  `health_verified=None` so a consumer can see verification did not run.

`HostOutcome::new` keeps its current `ok` semantics (`Completed`/`Prepared` →
`ok=true`); it does **not** flip `ok` for `None`. The deploy flow sets `health_verified`
explicitly: `Some(checker present ? actual : false)` and `None` when absent.

Add a test: with no checker bound, a successful deploy yields `health_verified=None`
(with `ok=true`); with a failing checker, `health_verified=Some(false)` and `ok=false`.

### AC2 — Remove dead code
- Delete `TargetSelection::select_filtered` and its `#[cfg(test)]` tests (grep-confirmed
  zero production callers).
- Delete `DeploymentStateMachine::name()` and `can()` and their `#[allow(dead_code)]`
  (zero callers). If observability/persistence needs a name later, re-add then.

### AC3 — ISP-lean audit port
Change `AuditStorePort::record` to take the host name:

```rust
async fn record(&self, host_name: &str, outcome: &str) -> Result<(), NodError>;
```

Update the confirmable calling sites (grep `record(` on the audit store / use case).
`JsonAuditStore::record` uses `AuditEntry::new(host_name, ...)` directly. `host`
parameter of the port is removed; `entries(host: Option<String>, ...)` is unchanged.
The `HostEntity` import in audit_store.rs is removed if now unused.

### AC4 — Consistent health error naming
Rename the `NodError::HealthCheck` variant finite scope **or** add an alias so the name
is consistent. Preferred: rename constructor-only inconsistency by keeping the variant
`HealthCheck` (PascalCase enum variants are idiomatic Rust) and **remove the bespoke
`healthcheck` constructor**, routing call sites to a consistent pattern. Verify: after
the change, grep shows no `NodError::healthcheck(` remains and the `HealthCheck` variant
is constructed/matched consistently (single shape). If any call site relied on the
constructor's ergonomics, provide `NodError::health_check(...)` (snake_case, matching
`deployment`/`config`/`evaluation`) and migrate callers.

### AC5 — Shared plan construction
Add to `src/domain/plan.rs`:

```rust
impl DeploymentPlan {
    /// Builds a plan for `hosts` without resolving closures (dry-run / preview).
    pub fn from_hosts(hosts: Vec<HostEntity>, action: DeploymentAction) -> DeploymentPlan
}
```

`from_hosts` produces `TargetPlan { host_name, action, new_closure: None,
current_closure: None }` for each host and wraps in `DeploymentPlan { targets, options:
DeploymentOptions::default_for(action) }` (or a provided `options`). Have
`deploy_fleet::plan_for` delegate to it. `generate_plan::plan` keeps building closures
but reuses the same loop/struct shape (it differs because it resolves closures).

### AC6 — Shared bounded runner (DRY concurrency)
Extract the semaphore + `JoinSet` pattern into one helper in `deploy_fleet.rs` (or a
small application helper), e.g.:

```rust
async fn run_bounded<I, F, Fut>(items: I, concurrency: usize, f: F) -> Vec<HostOutcome>
where
    I: IntoIterator<Item = HostOutcome-input>,
    F: Fn(Item) -> Fut,
    Fut: Future<Output = HostOutcome>,
```

Implementing the generic collector is optional; at minimum `build_only` and `run_wave`
must share the same semaphore/JoinSet scaffolding rather than each hand-rolling it. If a
clean shared signature is not reachable without fight-with-the-borrow-checker, factor the
smallest reusable piece (e.g. a `spawn_bounded` that spawns a vec of closures with a
semaphore and joins). Prefer the option that removes duplication without degrading
clarity.

## Out of Scope
- Wrapping hard-failure on missing health checker (intentionally NOT done — see AC1).
- P5 adapter hardening (quality gate, systemd, ssh quoting, eval propagation).
- Real dashboard dispatch, SSH-field further consolidation — separate waves.

## Verification
- `cargo test` — all pass (existing + AC1/AC2/AC3 tests).
- `cargo clippy --all-targets -- -D warnings` — zero warnings (confirmed by the
  dead-code removals).
- `cargo fmt --check`.
- `grep -rn "select_filtered" src/` → no production references (AC2).
- `grep -rn "\.can(\|state_machine.*name()" src/` → removed (AC2).
- `grep -rn "record(" src/application src/infrastructure src/commands` → new `&str`
  signature consistently (AC3).
- `grep -rn "healthcheck(" src/` → none (AC4).
