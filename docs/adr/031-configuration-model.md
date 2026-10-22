# ADR-031: Configuration Model

- **Status:** Proposed
- **Date:** 2026-10-22
- **Deciders:** maintainers
- **Supersedes:** the ad-hoc parts of ADR-004 (four-tier precedence) that mixed
  fleet declaration with process operation. ADR-004 still owns host overrides.

## Context

Three attempts in one day showed the problem: the daemon took scalars from CLI
flags, the fleet module tried to render a TOML file for provider declarations,
and the provider protocol added JSON. The result was three formats for one
question and a build failure caused by a test dependency, not by the design.

The question is not "which format". It is: **which source answers which
question, and what happens when two answer it?**

## Decision

### 1. Two scopes, not four tiers

Configuration answers two different questions, and they must not share a
precedence list:

| Scope | Question | Changes | Source |
|---|---|---|---|
| **Fleet** | What exists? Hosts, providers, desired resources | With a commit | Flake or file, never both |
| **Operation** | How does this process run? Interval, concurrency, log, state dir | Per host or run, without a commit | CLI > env > file > defaults |

Mixing them produces the precedence puzzles ADR-004 tried to solve. Separating
them removes the puzzle: a fleet question has exactly one source, an operational
question cascades.

### 2. Nix is the native source for the fleet

`nod` evaluates the flake it already evaluates for host discovery. The contract
is one top-level output:

```nix
# flake.nix
nod = {
  schema = 1;
  providers = {
    cloudflare = {
      command = "${pkgs.nod-provider-cloudflare}/bin/nod-provider-cloudflare";
      settings = { zone = "example.com"; token_env = "CLOUDFLARE_API_TOKEN"; };
      desired = ./desired/cloudflare.json;
      apply = false;
      order = "independent";
    };
  };
};
```

- No serialization, no generated TOML, no second parser: the module system types
  the declaration, and `nod` reads it with `nix eval --json`.
- The daemon evaluates the fleet once per revision, in the same evaluation it
  already performs for hosts.
- `schema` is checked before use; an unknown major is a configuration error.

### 3. A file is the native source for non-Nix users

The same structure, hand-written:

```toml
[providers.cloudflare]
command = "/run/current-system/sw/bin/nod-provider-cloudflare"
desired = "./desired/cloudflare.json"
apply = false

[providers.cloudflare.settings]
zone = "example.com"
token_env = "CLOUDFLARE_API_TOKEN"
```

Relative paths resolve against the file's directory, never the process cwd.

### 4. Two fleet sources are an error, not a merge

If a flake output and a configuration file both declare providers, `nod`
refuses with both paths named. Two declarations of one fleet are two truths,
and merging them invents a third.

### 5. Operation cascades

```
CLI flag > NOD_* environment variable > [runtime] in the file > built-in default
```

The set is deliberately small and flat: `flake`, `config`, `state-dir`, `log`,
`json`. Nested provider settings are not expressible through environment
variables; env is an operational surface, not a second configuration language.

### 6. Formats by purpose

| Purpose | Format | Why |
|---|---|---|
| Human configuration | TOML | comments, readable diffs |
| Machine data: protocol, desired state, audit, plans, state | JSON | every language reads it, Nix can emit it |
| Fleet declaration in Nix | the flake | typed by the module system |

Never generated: a TOML file rendered from Nix is a serializer, a second schema
and a build artifact for something the flake already says.

### 7. Secrets are references

Provider settings carry references, never values:

```toml
[providers.cloudflare.settings]
token_env = "CLOUDFLARE_API_TOKEN"
```

The core resolves the reference at run time and passes the value to the provider
through its environment or a file descriptor, never argv. A provider never logs
its settings.

### 8. Provenance is part of the interface

`nod config` prints every effective operational value with its source:

```
interval   60   cli
state-dir  /var/lib/nod   file
log        info  default
```

For the fleet, the source is the flake or the file, and `nod config` names it
once. A value whose origin is unknown does not exist.

## Consequences

- The daemon gains one evaluation (`nix eval .#nod`) per revision, on top of the
  host evaluation it already performs.
- `nod` must publish and version the `nod` flake output schema; third parties
  write against it.
- The NixOS module no longer renders provider configuration. It may still pass
  operational flags (interval, targets), which are per-process by nature.
- Non-Nix users get the same model through one file; nothing in the core assumes
  a flake is present.
- The file parser must reject unknown keys: a typo is an error, not a default.

## Rejected alternatives

- **Render TOML from Nix.** A serializer, a duplicated schema, and a generated
  artifact for data the flake already holds. Failed in practice.
- **Declare the fleet through CLI flags.** Unscalable for nested settings, and
  the unit becomes the only place the truth lives.
- **Merge flake and file.** Produces a third truth neither source states.
- **Environment variables for nested settings.** Turns env into a configuration
  language with no validation and no diffs.

## Implementation

1. `nod config` with provenance over the operational scope.
2. The `nod` flake output: schema, validation, `nix eval` reader.
3. Strict TOML parsing (`deny_unknown_fields`) and path resolution relative to
   the file.
4. Environment mapping for the five operational keys.
5. SIGHUP reload for file and environment; flags stay immutable per process.

## Open questions

- Does the fleet declaration belong at the flake root, per host, or both?
  (Root is fleet-wide; per host would let a host override its own provider set.)
- Should `nod config` be able to write (`nod config set`), or stay read-only?
- How does a non-Nix fleet express host inventory without duplicating the
  topology? A minimal `[hosts.<name>]` block, or an external inventory file?
