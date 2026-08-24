# Specification: Automatic Deployment Audit Trail Recording

## Status
Approved (ADR-013)

## Motivation
Deployments performed with `nod switch`, `nod test`, `nod boot`, and `nod rollback` must automatically record their outcomes into the persistent audit trail (`history.json`), allowing operators to inspect fleet history via `nod audit`.

## Acceptance Criteria

### AC1: Composition Root Default Binding
- `src/commands/wiring.rs::production` must bind `JsonAuditStore` as `AuditStorePort` on the constructed `AppContext`.

### AC2: Lifecycle Execution Recording
- `src/commands/lifecycle.rs::execute_lifecycle` must record each completed, rolled back, or failed host into the audit store when `dry_run` is false.
- When `dry_run` is true, no entries shall be recorded to the audit store.

### AC3: Rollback Execution Recording
- `src/commands/rollback.rs::execute` must record the targeted host with outcome `"rolled_back"` into the audit store upon successful rollback.

### AC4: Non-Fatal Logging
- If writing to the audit store fails (e.g. read-only filesystem or disk full), the error must be logged via `tracing::warn!` without failing the deployment.

### AC5: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Code format verified by `cargo fmt --check`.
