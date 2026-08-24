# Spec: Lifecycle Command Consolidation

**Module:** `src/commands/lifecycle.rs`, `src/commands/switch.rs`, `src/commands/test.rs`, `src/commands/boot.rs`, `src/commands/mod.rs`.
**References:** ADR-005 (Fleet Concurrency), ADR-006 (Unified Target Selection), ADR-008 (Composition Root), ADR-010 (Lifecycle Command Execution Consolidation).

## Problem

`nod switch`, `nod test`, and `nod boot` have identical orchestration workflows in the presentation layer (~60 lines of duplicate logic in each module), varying only in the concrete `DeploymentAction` (`Switch`, `Test`, `Boot`) and minor CLI flag default mappings (`--dry-run`, `--on-error`).

## Decision & Acceptance Criteria

### AC1 — Centralized Lifecycle Dispatcher
- Create `src/commands/lifecycle.rs` containing `execute_lifecycle(ctx, flake_path, action, params)`.
- Define `LifecycleParams` holding `target`, `tag`, `role`, `all`, `dry_run`, `concurrency`, `strategy`, `batch_size`, `fail_fast`, `auto_rollback`, `verbose`, `quiet`.

### AC2 — Thin Command Delegates
- `src/commands/switch.rs`, `src/commands/test.rs`, and `src/commands/boot.rs` delegate to `execute_lifecycle`.
- Preserve existing CLI interface, parameter signatures, and error semantics (100% backward compatible with existing unit and integration tests).

## Verification
- `cargo test` — all existing test suites continue to pass.
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check` — clean formatting.
