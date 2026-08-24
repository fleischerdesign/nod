# ADR-013: Automatic Deployment Audit Trail Recording

## Context
`AuditStorePort` and `JsonAuditStore` were designed in ADR-003 and ADR-008 to persist deployment outcomes to disk (`~/.local/share/nod/history.json`).
However, `wiring::production` previously omitted binding `JsonAuditStore` to `AppContext` by default, treating it as an opt-in component wired only within `Commands::Audit`. Consequently, deployment executions (`switch`, `test`, `boot`, `rollback`) did not persist their outcomes to the audit trail, leaving `nod audit` empty even after active deployments.

## Decision
1. **Composition Root Binding**: Bind `JsonAuditStore` as the default `AuditStorePort` in `src/commands/wiring.rs::production`.
2. **Lifecycle Audit Persistence**: In `src/commands/lifecycle.rs`, when running a real deployment (`!dry_run`), record the final outcome of each target host (`completed`, `failed`, `rolled_back`) into `ctx.audit_store()`.
3. **Rollback Audit Persistence**: In `src/commands/rollback.rs`, upon successful rollback execution, record the host with outcome `"rolled_back"` into `ctx.audit_store()`.
4. **Resilience**: Audit recording failures must be logged with `tracing::warn!` and must not fail an otherwise successful deployment.

## Consequences
- **Positive**: All deployment lifecycle events are automatically recorded into the persistent audit trail.
- **Positive**: `nod audit` provides immediate historical observability without requiring manual operator intervention.
- **Positive**: Clean Architecture & DRY: Composition root provides the complete standard graph across all commands.
