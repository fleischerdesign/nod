# ADR-004: Configuration Hierarchy (4-Tier Precedence)

- **Status:** Accepted
- **Date:** nod v2.0.0 transformation
- **Deciders:** nod maintainers

## Context

Nod needs configuration that describes *how* to deploy: which hosts are in scope, default targeting (`local` vs `all` vs a named host), SSH profile values (host/user/port), deployment behavior (verbosity, concurrency, rollout strategy), and verification policy.

Today the CLI is the only config: every command sits on positional+flag arguments (`target`, `flake`, `verbose`, `quiet`), and runtime constants are hard-coded (e.g. `target_user = "root"`, `target_port = 22`, `is_local` resolved by hostname). There is **no durable host metadata** beyond what Nix evaluates; nothing is overridable without rebuilding/editing a flag, and defaults cannot be tuned per flake.

As nod grows (fleet, state machine, concurrency, rollout), a single flag stream cannot express the needed control. We need a deterministic precedence that operators can reason about and document.

## Decision

Introduce a **four-tier configuration hierarchy**, with higher tiers overriding lower ones. Resolution merges tier-by-tier, last write wins, for each option:

```
    1. CLI flags            (highest)
    2. Local .nod.toml      (per-invocation config in the working dir / flake root)
    3. Flake metadata       (nixosConfigurations entries seen at evaluation)
    4. System defaults      (lowest; compiled-in, documented fallbacks)
```

Practical rule: **“Anything a CLI flag can set, a config file can set one tier down; anything the flake declares can be overridden by the user TOML; the fallback is the built-in default.”**

The **default flake root** resolves through its own cascade, independent of the four tiers above: CLI `--flake` > `[defaults].flake` > the system marker `/etc/nixos/flake.nix` > the working directory. The marker tier mirrors nixos-rebuild's documented default — "the directory containing the target of the symlink `/etc/nixos/flake.nix`, if it exists" — so `nod <cmd>` finds the system flake from any cwd without a Nix option (a marker is a convention, not config). Unlike `nixos-rebuild`, the marker is resolved on the control host where nod runs, so it resolves the controller's `/etc/nixos/flake.nix` even when the deployment targets are remote; fleets must pin a remote target's root with `--flake` or `[defaults].flake`.

### What each tier supplies

- **CLI flags** — temporary, per-run overrides; `--flake`, `--target`, `-v/--verbose`, `-q/--quiet`, and (once ADR-005 lands) `--strategy`, `--batch-size`, `--concurrency`.
- **Local `.nod.toml`** — persistent per-project scope: default target, SSH profiles per host, rollout/concurrency, verification on/off, log settings. Discovered by walking up from the flake root / cwd.
- **Flake metadata** — schema implied by `nixosConfigurations.<name>.deployment.*` and `networking.hostName`; supplies per-host `target_host`, `target_user`, `target_port`, `role`, and local/remote classification. This is read-only engine input, not something the operator sets by hand.
- **System defaults** — compiled fallbacks: user `root`, port `22`, local = `hostname == this machine`, default target `local`, verification **on**, concurrency serial until ADR-005.

### Precedence example (one host)

```
1 CLI:   --target jello --concurrency 4
2 TOML:  [hosts.jello] user="philipp" port=2222  rollout="canary"
3 Flake: target_host="10.0.0.8", role=Server
4 Defaults: user="root", port=22, verify=true

merged:  target=jello, user="philipp"  (TOML, overriding flake/root default),
         port=2222 (TOML beats built-in 22), concurrency=4 (CLI top tier)
```

Each option resolves independently; missing tiers simply fall through.

## Consequences

### Positive

- **Predictability:** precedence is stable, documented, and self-describing to operators.
- **Path to fleet control:** `.nod.toml` becomes the natural place to declare rollout/concurrency for a whole flake (without a battery of CLI flags).
- **Flake stays the source of truth for what a host is**; TOML supplies the *policy over* that topology.
- Migration headroom: current hard-coded and `discover_hosts`-local defaults migrate up cleanly (root/22/local detection first).

### Negative / Trade-offs

- **One more configuration surface** to parse and document.
- **CLI versus TOML ambiguity** for new options: when a flag and a TOML key both exist, precedence is fixed (CLI wins) — reduces surprise but forecloses the "TOML example to override flag” pattern.
- Flake metadata derives from Nix evaluation (the evaluator adapter may be slow); re-reading host topology per run is a real cost.

## Compliance

- The `AppContext` resolves a *merged* typed `Config` once per run, exposing the winning value per option; **no module outside config reads `.nod.toml` or hard-coded fallback**.
- Higher tier always wins, independent per option. Gherkin `foundation.spec.md` adds a scenario asserting `CLI > TOML > flake > default` for a representative option.

## Residual SSH-field duplication (documented decision)

The 10-field SSH connection set intentionally appears in **three** structs after
consolidation (`ssh-field-consolidation.spec.md`, ADR-008 composition root):

| Type | Serialization shape | Where |
|---|---|---|
| `SshOverrides` | TOML, snake_case, `#[serde(default)]` on `extra_ssh_args` | `toml_config.rs` |
| `SshConnectionOverrides` | domain flat (embedded via `#[serde(flatten)]` in `HostOverrides`/`FleetDefaults`) | `domain/config.rs` |
| `SshProfileConfig` | Nix `config.nod` JSON, camelCase + aliases | `domain/config.rs` |

These are **not** DRY violations to be removed: each shape is bound to a distinct
serialization source (TOML keys, domain merge output, Nix JSON with aliases). Folding
them into one struct would force a serde shape break in at least one boundary. Keep them
separate; if a future change reconciles two sources, that is a deliberate new ADR, not a
cleanup. (ADR-007 keeps the runtime `SshProfile` as a fourth, non-optional value object.)

- ADR-001 (AppContext: config resolution), ADR-002 (`ConfigError`), ADR-005 (TOML carries concurrency/rollout); `../spec/foundation.spec.md`.