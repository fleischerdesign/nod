# ADR-034: Declared deletions

- **Status:** Accepted
- **Date:** 2026-10-23
- **Deciders:** maintainers
- **Refines:** ADR-030 §7 (deletion policy). Changes what the cap counts, not
  the wire protocol.

## Context

The deletion policy has two guards: `protect`, a list of names that may never
be deleted, and `max_deletions`, a per-run cap. The cap exists to bound the
blast radius of a **declaration bug** — an empty desired file, a filter that
drops half the resources, or an incomplete first declaration would otherwise
delete every owned resource in one run.

The shipped rule counts every deletion and refuses the plan as a whole once the
count exceeds the cap. That conflates two different things:

- A resource that **vanished** from the declaration. Nobody wrote "delete
  this"; its absence is exactly the bug the cap is meant to catch.
- A resource the declaration marks **absent**. The operator wrote "this must
  not exist"; the deletion is the intention, reviewed like any other line.

With `max_deletions = 0` — the safe default — the second case is impossible: a
tombstone refuses the whole plan, including creates and updates that have
nothing to do with it. Raising the cap to let it through weakens the guard for
everything else in that run, and a bug that drops one resource then deletes it
silently.

## Decision

### 1. A declaration has two lists

`resources` must exist; `absent` must not. Both carry the same resource shape
(`kind`, `id`). The provider never sees `absent`: it plans from `resources`
alone, and the delete it proposes for an owned resource outside that list is
reclassified by the core.

A separate list is deliberate. A tombstone inside `resources`, marked with an
attribute, would be created by any provider that does not understand the
marker — a data-loss bug in exchange for a shorter schema.

### 2. The cap counts implicit deletions only

| Kind | Definition | Cap |
|---|---|---|
| declared | a delete whose `kind`/`id` appears in `absent` | not counted |
| implicit | every other delete of an owned resource | counted against `max_deletions` |

The plan is still approved or **refused as a whole**: if the implicit
deletions exceed the cap, nothing applies. The no-half-apply rule from ADR-030
is unchanged.

### 3. `protect` applies to both

A protected name is never deleted, even when the declaration marks it absent.
The protect list guards against a bug in the tombstone list; making it
conditional on the declaration would invert its purpose.

### 4. An unsatisfiable declaration fails before anything applies

| Declaration | Result |
|---|---|
| `absent` names a resource the fleet does not own | configuration error — the fleet cannot delete what it does not control, and silence would hide the mistake |
| `absent` names an owned resource the provider planned no deletion for | configuration error — the provider cannot carry the declaration out |
| `absent` names a resource that does not exist | satisfied, no change |

## Consequences

- A rename is one declared deletion plus one create, and works with
  `max_deletions = 0`.
- A retirement works without touching the cap.
- A declaration bug that drops resources still refuses the whole plan.
- `nod provider status --json` reports `declared_deletions` and
  `implicit_deletions` for the proposed plan, so the split is visible before
  approval.
- Providers are unchanged. The desired-state file format gains an optional
  `absent` array; an older core ignores it, so the Nix projection and the
  binary must ship together.
