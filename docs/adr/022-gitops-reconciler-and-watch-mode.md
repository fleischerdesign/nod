# ADR-022: GitOps Reconciler and Interactive Watch Mode (`nod watch`, `nod sync`, `nod daemon`)

## Context
Operators need modern automation and rapid feedback loops:
1. **Interactive Watch Mode** (`nod watch`): Live developer feedback loop that watches the repository directory for `.nix` / `.toml` changes and instantly triggers evaluation, build validation, and diff previews without applying them.
2. **Pull-Based GitOps Reconciler** (`nod sync` / `nod daemon`): An automated daemon service running on individual nodes or management servers that periodically pulls from the upstream Git repository, builds the target closure into the Nix store, and safely executes `nod switch` with automated health checks and rollbacks.

## Decision
1. **Domain Layer (`src/domain/watch.rs`)**:
   - `WatchOptions` (debounce duration, poll interval).
   - `SyncOptions` (remote, branch, interval, dry-run, once).
   - `SyncReport` (commit hash, changes detected, applied, error).
2. **Application Layer**:
   - `WatchFlakeUseCase` (`src/application/use_cases/watch_flake.rs`): watches files for changes and executes plan preview loops.
   - `SyncDaemonUseCase` (`src/application/use_cases/sync_daemon.rs`): pull-based GitOps loop.
3. **Presentation Layer**:
   - `nod watch [TARGET] [--flake <PATH>] [--interval <SECS>]`
   - `nod sync [--remote <NAME>] [--branch <BRANCH>] [--once] [--interval <SECS>]`
   - `nod daemon [--interval <SECS>]`

## Consequences
- **Positive**: Blazing fast feedback cycle during Nix configuration editing (`nod watch`).
- **Positive**: First-class pull-based GitOps reconciliation native to `nod`.
- **Positive**: 100% test coverage and strict architectural isolation.
