# Spec: Parallel Flake Discovery & Subprocess Resilience

**Module:** `src/infrastructure/nix/cli_evaluator.rs`.
**References:** ADR-001 (Evaluator Port), ADR-005 (Fleet Concurrency), ADR-011 (Parallel Flake Discovery & Subprocess Resilience).

## Problem

`NixCliEvaluator::eval_host_metas` evaluated each host's metadata sequentially (`for name in host_names`), creating $O(N)$ sequential subprocess invocations. On large fleets, this caused noticeable lag during command startup and host selection.

## Decision & Acceptance Criteria

### AC1 — Concurrent Evaluation Pool
- In `NixCliEvaluator::eval_host_metas`, evaluate host metadata in parallel across a bounded `tokio::task::JoinSet` with a `tokio::sync::Semaphore`.
- Collect all per-host results and sort them to preserve deterministic host ordering matching `nixosConfigurations`.

### AC2 — Error Isolation & Strict/Degraded Semantics
- Individual evaluation failures on one host do not terminate evaluation of other hosts.
- Strict mode still hard-fails on the first error, and degraded mode skips failing hosts with the documented warning message (`AC6`).

## Verification
- `cargo test` — all existing test suites pass.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean formatting.
