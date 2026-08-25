# Specification: Day-0 Provisioning, Scaffolding & ISO Generation (`nod bootstrap`, `nod init`, `nod iso`)

## Status
Approved (ADR-021)

## Motivation
Enable seamless Day-0 operations including repository scaffolding, bare-metal bootstrapping via `nixos-anywhere` and `disko`, and bootable installer ISO generation.

## Acceptance Criteria

### AC1: Repository Scaffolding (`nod init`)
- `nod init [DIR]` must create a valid Nix flake with `flake.nix`, `.nod.toml`, and `hosts/` configuration templates.
- It must support `--template minimal`, `fleet`, and `server`.
- It must refuse to overwrite non-empty directories unless confirmed.

### AC2: Bare-Metal Bootstrapping (`nod bootstrap`)
- `nod bootstrap <TARGET> --ip <IP>` must resolve the target host in the flake and run `nixos-anywhere`.
- It must support `--disko`, `--no-kexec`, and `--debug` flags.
- When `--json` is provided, output must be structured JSON reporting status.

### AC3: Installation Media Generation (`nod iso`)
- `nod iso [TARGET]` must evaluate and build the bootable installer ISO closure for the specified host.
- When `--json` is provided, it must output the absolute path to the generated `.iso` / image.

### AC4: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Format verified by `cargo fmt --check`.
