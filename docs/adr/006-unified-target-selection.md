# ADR-006: Unified Target Selection Grammar & Set-Intersection Semantics

- **Status:** Accepted (target-state for `nod v2`)
- **Date:** nod v2.0.0 transformation
- **Deciders:** nod maintainers

## Context

Today host-operating commands accept **two incompatible target grammars**:

```rust
// nod switch web-01                 // exactly one positional target
// nod switch --all                          // or the sentinel "all" / "local"
// nod switch --tag prod --role server    // filters narrow a broad sentinel
```

`switch`, `status`, `diff` and `rollback` resolve a *single* positional target (or the
`all`/`local` sentinels); `check` additionally accepts `--tag`/`--role` filters. The
sentinel targets (`all`, `local`) are magic strings recognized inside
`TargetSelection::select`, not a grammar — and the per-command filters are
plumbed through ad-hoc `Option<&str>` arguments.

This split produces **cognitive friction**: an operator must remember which commands
take a positional target, which accept filter flags, and that `--all` is the only way to
address a fleet breadth-first. There is no way to say "every database host in the
prod fleet" as a single expression today; sequence `nod switch --all --tag db` is a
workaround that only *some* commands support. Worse, filters and target are combined by convention-only
(`TargetSelection::select_filtered` applies filters after the target with no documented
AND rule), so `nod switch web-01 --tag prod` resolves differently per command —
silently narrowing to nothing, or to a set the operator did not intend.

As the fleet grows (ADR-005) and the interactive/automated surfaces multiply, one
predictable, orderable target grammar shared by *every* host-operating command is a
prerequisite: the discover→select→deploy pipeline needs one way to ask
"*which hosts run this operation?*".

## Decision

Standardize **every host-operating command** on a single **Unified Target Selector**:

```
nod <command> [TARGET/GLOB] [--tag <TAG>] [--role <ROLE>] [--all]
```

- **`TARGET`**: exact hostname match first, else a shell-style **glob** (`*`, `?`,
  `[...]`). A target that is neither an exact name nor a valid glob is an error.
- **`--tag <TAG>`** and **`--role <ROLE>`**: filter criteria; a target is selected only if
  it holds the tag / matches the role.
- **`--all`**: the empty/identity target — "the whole fleet, whatever it is" — and may
  still be narrowed by `--tag`/`--role`.

### Selection semantics: boolean AND

If *more than one* criterion is supplied, the criteria are combined with
**boolean AND (set-intersection)** — each criterion *narrows* the previous result.
The set of selected targets is:

```
selected = hosts
         ∩ name==TARGET (exact)  ∪  name ~= GLOB
         ∩ has_tag(TAG)
         ∩ role == ROLE
         ∩ fleet (when --all is the only selector)
```

Example: `nod exec "db-*" --tag prod` addresses *only the prod-tagged database
hosts*. `--all` alone is the whole fleet; `--all --role edge` means
"every host with the edge role".

`--all` is **not a negation** of a target; it is the empty positional selector. The
implementation must keep the selector **order free**: `--tag prod --role web --all`
resolves identically to `--all --tag prod --role web`.

### Single-target constraint

For commands with a **single terminal** (`nod ssh`, `nod repl`), the selector must
resolve to **exactly 1 host**:

- **0 hosts matched** → clean error listing the criteria, "no hosts matched".
- **>1 hosts matched** → clean error listing the matched hosts, telling the
  operator to target a named host or narrow the filters.

All other commands (`switch`, `rollback`, `status`, `diff`, `check`, `plan`) admit
0..n hosts in the resolved set.

### Compatibility and migration

- `--all`, `--local`, and the `all`/`local` sentinels remain accepted as an alias to
  `--all` (resp. the local hostname) and land on integration. Sentinel strings are
  positional sugar, *not* magic specials: `nod switch local` continues to mean "this machine".
- The filter flags use the same tier grammar as every configuration (ADR-004): the
  CLI flag is tier 1 today; the same criteria can arrive from `.nod.toml` (tier 2) via a
  future `[targets]` section — the selector function must stay pure so tiers can feed it.

## Consequences

### Positive

- **One grammar everywhere**: globs + tag/role/`--all` work identically on every host-operating command; the "which hosts?" question has exactly one answer.
- **Composition without surprises**: a target and filters compose by AND, so an operator can always
  narrow a broad intent (`nod check "db-*" --tag prod`) in a way that will not diverge
  between commands.
- **Clean error surface**: single-host commands fail loudly (0 or >1 hosts) instead of
  first-host accidents; the multi-host commands list the same resolution.
- **Testable seam**: `TargetSelection` becomes a *pure*, falsifiable resolver (exact match → glob →
  filters → AND) that runs against the discovered host list without Nix/SSH (`../spec/unified-target-selection.spec.md`).

### Negative / Trade-offs

- **Glob ambiguity**: a literal hostname that also matches a glob is shadowed by the exact match of
  (exact beats glob). Operators who genuinely want a set narrower than the exact name must
  write a glob that excludes it (`web-0?` vs `web-01`).
- **Custom selector cost**: every command now resolves its hosts against full discovery once per run; the
  fleet list is small (dozens), so a naive `O(n × patterns)` pass is fine; the selector keeps the
  candidate hosts and filters in memory but never orders or mutates them.
- **Shell interference**: an unquoted glob is expanded by the operator's shell before `nod`
  sees it. The grammar treats an already-expanded set of names as a *list of exact name targets*
  (implicit OR) — which composes with `--tag`/`--role` via the same AND. Operators who overlap
  with filenames must quote (`nod exec "db-*" ...`).
- **`--all` vs `all`**: legacy scripts using `nod switch all` keep working; the sentinels
  are a compatibility alias, not a second grammar.

## Compliance / Verification

- The behaviour pins `../spec/unified-target-selection.spec.md`: globbing, AND-composition,
  exact-match precedence, and the single-host error (0 or >1 matches).
- Exact-name precedence: a target that is simultaneously a live glob and an exact name always resolves
  to the exact host only (never the set).
- Set size for single-terminal commands (`ssh`, `repl`) is exactly 1 in every integration
  regression; every other command admits a 0..n set.
- The selector stays in the Application layer (`src/application/selection.rs`) and is unit-tested
  (mock discovered hosts) without Nix/SSH.

## Related

- ADR-001 (Ports/Adapters — selection is pure Application logic), ADR-004 (config
  tiers route the criterion values), ADR-005 (fleet rollout consumes this set —
  scheduling order is owned by the rollout strategy, not by the selector),
  spec `../spec/unified-target-selection.spec.md`.</parameter>
</invoke>