# Specification: Store & Generations Management

## Status
Approved (ADR-015)

## Motivation
Provide operators with comprehensive management of NixOS profiles, generation history, garbage collection, and closure copying across the fleet.

## Acceptance Criteria

### AC1: Generation Inspection (`nod generations`)
- `nod generations` must query `/nix/var/nix/profiles/system*` across all resolved targets.
- For each generation, it must report:
  - Generation number (e.g. `120`)
  - Active status (`*` / `current`)
  - Creation date formatted in UTC (`YYYY-MM-DD HH:MM:SS UTC`)
  - Store closure path
- With `--json`, it must output a structured JSON array of host generations.

### AC2: Remote & Local Garbage Collection (`nod gc`)
- `nod gc` must execute garbage collection across resolved targets under `--concurrency N`.
- When `--keep N` is provided, it must retain at least the latest $N$ generations.
- When `--older-than X` (e.g. `14d`, `30d`) is provided, it must delete generations older than $X$.
- When `--dry-run` is specified, it must preview reclaimable disk space without deleting closures.
- Results must be summarized with host names and freed space.

### AC3: Closure Copying (`nod copy`)
- `nod copy` must evaluate and build the target system closure and copy it to the target host (`nix copy --to`) without activating or changing the boot profile.
- When `--from URL` or `--to URL` is provided, it must copy to/from the custom binary cache / remote store URI.

### AC4: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Code format verified by `cargo fmt --check`.
