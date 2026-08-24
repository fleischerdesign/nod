# Spec: Flake-tier SSH Resolution in the Config Store (ADR-004/ADR-007)

**Module:** `src/infrastructure/config/toml_config.rs` (and its tests).
**References:** ADR-004 (`docs/adr/004-configuration-hierarchy.md`),
ADR-007 (`docs/adr/007-effective-connection-profile.md`).
**Config Cascade:** CLI > `.nod.toml` > Flake (`config.nod.*`) > System Defaults.

## Problem

ADR-004 declares a four-tier configuration cascade for every connection option,
including the SSH identity:

| Tier | Source | Winner on collision |
|---|---|---|
| 1 | CLI flags (`--user`, `--port`, `--identity-file`) | highest |
| 2 | `.nod.toml` (`[hosts.<name>.ssh]` > `[fleet]` > `[defaults]`) | |
| 3 | Flake metadata (`config.nod.ssh.*`) | |
| 4 | Compiled-in defaults (`root` / `22`) | lowest |

`TomlConfigStore` is the `ConfigStorePort` bound in production composition.
Its `resolve()` and `apply_to()` currently honor tiers 1, 2 and 4 for SSH, but
**tier 3 is never overlaid**: the flake-metadata values materialized on
`HostEntity.nod_config.ssh` (`identity_file`, `proxy_jump`, `proxy_command`,
`sudo`, `timeout_secs`, `connect_timeout_secs`, `extra_ssh_args`,
`allow_insecure`) never reach the resolved `SshProfile`. Only `user` and `port`
happen to flow through the evaluator's direct `target_user`/`target_port`
assignment.

Concretely: a flake that sets `config.nod.ssh.identityFile = "~/.ssh/deploy-key"`
(in Nix spelling `nod.ssh.identityFile`, emitted camelCase by the evaluator) is
connected to as `root@<target>` **without** that identity file. This is silent
configuration loss — the exact class of defect ADR-007 sets out to eliminate.

## Decision

`TomlConfigStore::resolve` must overlay tier 3 (`host.nod_config.ssh`) as the
SSH baseline, beneath `.nod.toml` (tier 2) and CLI (tier 1), so that the
effective profile honors every tier of the documented cascade. `apply_to`
already derives user/port from the resolved profile and remains correct.

Implementation shape (DRY): convert `SshProfileConfig` → the store's internal
override type once, and reuse the existing `Merged::overlay` merge (no new
merge semantics, no per-host special-casing). This keeps the adapter agnostic:
any flake that declares `config.nod.ssh.*` is honored without touching nod's
host/tooling knowledge.

## Acceptance Criteria

### AC1 — Flake SSH identity is resolved when no lower override exists
Given a `HostEntity` whose `nod_config.ssh.identity_file` is set to
`/flake/id_rsa` and **no** `.nod.toml` and **no** CLI identity override,
`resolve(host)` returns a profile whose `identity_file()` is
`Some("/flake/id_rsa")`.

Tests cover at least `identity_file` and `proxy_jump` (representative of the
optional connection fields); the overlay applies uniformly to all
`SshProfileConfig` fields.

### AC2 — `.nod.toml` still beats flake metadata
Given the same flake metadata as AC1 **and** a `.nod.toml` `[hosts.<name>.ssh]`
identity of `/toml/id_rsa`, `resolve(host)` returns `Some("/toml/id_rsa")`.
Tier 2 overrides tier 3.

### AC3 — CLI still beats `.nod.toml` and flake metadata
Given flake `/flake/id_rsa`, `.nod.toml` `/toml/id_rsa`, and CLI
`--identity-file /cli/id_rsa`, `resolve(host)` returns `Some("/cli/id_rsa")`.
Tier 1 overrides tiers 2 and 3.

### AC4 — Absent flake metadata is a no-op
A host with default `NodConfig` (all `SshProfileConfig` fields `None`) resolves
exactly as today: compiled defaults (`root`/`22`) with no identity, identical to
the pre-change behaviour. No existing behaviour regresses.

### AC5 — `apply_to` continues to derive user/port from the resolved profile
`apply_to` remains driven by `resolve()`; no change to its semantics.

## Out of Scope

- Changing the Nix module surface (`options.nod.*`) — it already declares the
  full `ssh` subtree.
- Altering tier precedence order (ADR-004 is authoritative).
- Host/tooling-specific identity wiring — the fix is purely cascade-completion
  in the config store and is flake-agnostic.

## Verification

- `cargo test` — all pass (existing + new AC1–AC4 tests).
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- `nix flake check` on a consumer flake (e.g. `/etc/nixos`) confirms the module
  gate is unaffected.
