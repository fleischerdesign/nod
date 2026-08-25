# Specification: GitOps Reconciler and Interactive Watch Mode (`nod watch`, `nod sync`, `nod daemon`)

## Status
Approved (ADR-022)

## Motivation
Provide developers with live change previews (`nod watch`) and automated pull-based GitOps synchronization (`nod sync` / `nod daemon`).

## Acceptance Criteria

### AC1: Live Watch Mode (`nod watch`)
- `nod watch [TARGET]` must watch the flake directory for file modifications (`.nix`, `.toml`).
- On modification, it must re-evaluate the configuration and display the generated plan and diff.
- It must not crash on evaluation errors and keep watching.

### AC2: GitOps Reconciler (`nod sync` / `nod daemon`)
- `nod sync` must fetch the latest commits from upstream Git remote.
- When new commits are detected, it must build the system closure in the local Nix store.
- If the build succeeds, it activates the configuration and performs health verification with auto-rollback on failure.
- `--once` must perform a single reconciliation run and exit.
- `nod daemon` must run continuous reconciliation at the configured interval.

### AC3: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Format verified by `cargo fmt --check`.
