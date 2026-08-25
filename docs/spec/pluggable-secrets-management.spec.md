# Specification: Pluggable Secret Verification and Fleet Rekeying (`nod secret`)

## Status
Approved (ADR-018)

## Motivation
Provide operators with pre-flight secret verification and fleet rekeying tools across heterogeneous NixOS setups (sops-nix, agenix, custom, none).

## Acceptance Criteria

### AC1: Pre-Flight Secret Verification (`nod secret check`)
- `nod secret check [TARGET/GLOB]` must resolve target hosts matching standard selector criteria (`[TARGET]`, `--tag`, `--role`, `--all`).
- It must auto-detect the secret provider (`sops`, `agenix`, `custom`, `none`) for each host.
- For hosts without secrets, it must report `none` provider and pass cleanly without failing.
- For `sops` and `agenix` hosts, it must verify file decryptability and report invalid files with descriptive error details.
- When `--json` is provided, output must be valid JSON mapped per host.

### AC2: Secret Rekeying (`nod secret rekey`)
- `nod secret rekey [TARGET/GLOB]` must re-encrypt secrets for the target host's resolved provider.
- It must support `--dry-run` to preview rekeyed files without modifying disk.
- It must support `--no-backup` to bypass creation of `.bak` backup files.

### AC3: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Format verified by `cargo fmt --check`.
