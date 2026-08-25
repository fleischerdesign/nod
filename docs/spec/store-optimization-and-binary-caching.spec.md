# Specification: Store Optimization & Binary Cache Integration (`nod store optimize`, `nod cache push`)

## Status
Approved (ADR-019)

## Motivation
Equip operators with tools to optimize disk space via Nix store hardlink deduplication across the fleet, and push compiled system closures to remote binary caches.

## Acceptance Criteria

### AC1: Store Hardlink Optimization (`nod store optimize`)
- `nod store optimize [TARGET/GLOB]` must resolve target hosts matching standard selector criteria (`[TARGET]`, `--tag`, `--role`, `--all`).
- It must run `nix-store --optimise` or `nix store optimise` locally or via SSH.
- When `--json` is provided, output must be valid JSON mapped per host.

### AC2: Binary Cache Push (`nod cache push`)
- `nod cache push [TARGET/GLOB]` must compile the target host closure and push it to the configured or `--cache <URI>` destination.
- When `--cache` is omitted, it must fallback to `config.nod.build.cache` or `config.nix.settings.substituters`.
- When `--json` is provided, output must be structured JSON reporting the pushed closures.

### AC3: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Format verified by `cargo fmt --check`.
