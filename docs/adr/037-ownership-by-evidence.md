# ADR-037: Ownership by evidence

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-032 (adoption), ADR-035 §5 (ownership per kind), ADR-036
  (declared derivations).

## Context

ADR-035 fixes ownership per kind: a marker where the object has a free-form
field, a derivation from the parent where it has none. Authentik's `Application`
has no free-form field — only `meta_description`, `meta_publisher`, `meta_icon`,
`group` and `provider` — and a marker in `meta_description` would be visible in
the interface and editable by a human.

Ownership answers one question: may the fleet touch this resource? The marker is
one kind of evidence. A rule that produced the resource is another. A create the
fleet performed is a third. All three are facts the fleet holds, and none of
them requires the object to carry a field.

## Decision

### 1. One record, three kinds of evidence

The record — `providers/<provider>.ownership.json` — holds:

- `rules`: rule id → the resources the rule produced (ADR-036). This is also
  the previous set the deletion budget is measured against, and it carries
  provenance (`rule`, `member`) into the plan and the report.
- `created`: resources this fleet created and no rule derives. A marker-less
  kind cannot carry that proof itself.

Entries are `{kind, id}` pairs, never bare ids: an id collision across kinds
must not claim the wrong resource. `owns` answers from both lists.

### 2. The core applies the record before planning

An observed resource whose `(kind, id)` appears in the record is `owned`,
whatever the provider reported. The provider is unchanged — it reports the
marker it can see; the core owns the evidence. The same override applies to the
post-activation observation, so verification, `unowned` and the plan agree.

### 3. The record is load-bearing, and its loss is safe

Without the record, marker-less resources are foreign: the provider does not
update or delete them, and `unowned` reports them. The fleet does not guess
ownership from a slug, a name or a namespace. Recovery is an operator decision —
recreate the resource, or adopt it once it carries a marker — never an automatic
claim.

### 4. Evidence follows reality in one direction

`created` entries are dropped when the provider no longer reports the resource,
except for a create in the current run: pruning that would make a fresh resource
foreign before the API lists it, the one state the declaration cannot repair.
Rule entries are never pruned by an observation — they follow the declaration,
so a dry run cannot wipe the next run's budget.

A resource that is both derived and declared absent is refused: the two
instructions contradict each other.

### 5. Adoption stays marker-based

`nod provider adopt` still stamps a provider marker. A kind without one is
created by the fleet and recorded, not adopted after the fact. Adoption is for a
world that existed before this fleet; extending it to marker-less kinds needs an
explicit operator statement and its own decision.

## Consequences

- A kind without a free-form field becomes declarable, statically and derived,
  without a visible marker.
- The provider protocol does not change; the override lives in the core.
- The record is part of the ownership model, not only of the deletion budget; it
  is written next to the rollback plan and protected the same way.
- A lost record degrades to "foreign and reported", never to an update or a
  delete of something the fleet cannot prove it created.
- A stale `created` entry is inert until a resource with that id appears; if one
  appears, the fleet treats it as its own. The window requires a manual
  creation at an id the fleet once used.

## Alternatives

| Alternative | Why not |
|---|---|
| marker in `meta_description`/`meta_publisher` | user-visible, and the interface may edit it |
| namespace/slug heuristic | a foreign object with the right name would be claimed |
| provider-side marker field per kind | works, but the field does not exist for every kind and would be provider-specific configuration |
