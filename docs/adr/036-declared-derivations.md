# ADR-036: Declared derivations

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-030 (provider protocol), ADR-034 (declared deletions),
  ADR-035 (the provider owns Authentik). Adds a desired-state construct; no
  provider change.

## Context

A desired state is a static list. That is enough for everything the repository
knows at evaluation time, and it is the reason nod is predictable: the plan is a
function of the declaration.

Some resources cannot be listed at evaluation time because they follow a fact
that only exists at runtime:

- one instance per member of a directory group, where some members are declared
  seeds and others are created in the interface, by enrollment or by an LDAP
  sync;
- one record per device in an inventory that changes outside the repository;
- one credential per active service account.

Nix cannot evaluate those facts. A provider cannot invent desired state: it
reconciles what the declaration asks for. An external controller could, but it
would duplicate ownership, deletion policy, retry, audit and health — the second
truth this repository keeps removing.

The missing construct is a **rule**: a static, reviewed declaration that says
how a set of resources follows from an observed fact.

## Decision

### 1. A rule is the declaration

The desired state gains a `derive` list:

```json
{
  "schema": 2,
  "resources": [ … ],
  "absent": [ … ],
  "derive": [
    {
      "id": "openclaw-per-ai-member",
      "from": { "kind": "authentik.group", "id": "ai-users", "attribute": "users" },
      "template": {
        "kind": "openclaw.instance",
        "id": "{member}",
        "attributes": { "owner": "{member}", "group": "ai-users" }
      },
      "max_deletions": 0
    }
  ]
}
```

The effective desired set is `resources` ∪ expand(`derive`, observed). Nothing
else. The rule is static, versioned and reviewed like any other declaration.

### 2. The expansion is bounded and pure

- One source per rule: exactly one observed resource (`kind`, `id`) and one
  attribute.
- The attribute must be a list of strings. A group's `users`, a device
  inventory's names, a list of account ids — not arbitrary structures.
- The template substitutes `{member}` in `id` and in string attributes. No
  other syntax, no expressions, no conditionals.
- Expansion order: rules in declaration order, members sorted by value. The
  result is deterministic for a given observation.
- Duplicate ids — two rules producing the same `kind`/`id`, or a derived id
  colliding with a static resource — are a configuration error, not a merge.
- A derived resource is never a source for another rule (no chaining in v1).
  Chaining would need a topological expansion and a cycle rule; it is a
  separate decision.

### 3. An unavailable source is not an empty set

This is the rule that keeps the feature from becoming a footgun.

- If the observation of the source resource is missing from a **successful**
  observe, the plan is refused: the rule names a source that does not exist.
- If the attribute is missing or is not a list of strings, the plan is refused.
- If the observe itself failed, the run fails before expansion (today's
  behaviour); no derived set is computed.
- A source that exists with an **empty** list is a legitimate empty set. It
  produces an empty expansion, and the removals it implies are governed by the
  rule's deletion budget (below). An empty group is a fact; a timeout is not.

The distinction is load-bearing: a naive expansion that treats "no answer" as
"no members" would delete a fleet on the first timeout.

### 4. Provenance is part of the plan

Every expanded resource carries its origin in the core's model:

```
create openclaw.instance/alice   derived from authentik.group/ai-users, member alice
```

The provider never sees the provenance — it receives a normal resource. The
core owns the explanation, prints it in the plan and the report, and keeps it
for the audit record. A derived resource that cannot name its origin is a bug.

### 5. Derived deletions are declared, but budgeted

A member leaving the source removes its resource. That deletion is authorized
by the rule — but it was not reviewed individually, so it is bounded:

- Each rule declares `max_deletions` (default **0**): how many derived resources
  it may remove in one run. `0` freezes the set; the plan is refused and names
  the rule.
- A run that would exceed any rule's budget refuses the **whole plan**
  (ADR-030's no-half-apply rule).
- The core records the derived set per provider and rule in its state after a
  successful apply. A delete whose id is in that record is a derived deletion;
  every other delete stays **implicit** and counts against the global cap
  (ADR-034). The same record also carries ownership for kinds without a marker
  (ADR-037).
- Removing a rule from the declaration does not delete its resources: they stop
  being derived and become implicit deletions under the global cap.
- A lost or corrupt record degrades to "implicit" and "foreign": derived
  deletions become cap-protected, and a marker-less resource is no longer owned
  — safe and loud, never "delete everything" and never an unproven claim.

### 6. Schema version, so skew is loud

The desired-state file gains an optional `"schema"` (default `1`). `derive`
requires `2`. A core that does not know the declared schema refuses the file
instead of silently ignoring rules it does not understand. The same guard
applies to any future construct that changes the meaning of a declaration.

### 7. Configuration stays declarative

The rules are produced by the same projection that produces `resources` and
`absent` (Nix, contracts, inventory). They are reviewable in a diff, versioned
with the fleet, and have no plugin surface. A rule is data, not code.

### 8. Scope of this decision

- **In scope:** same-provider derivation. The source and the derived resources
  are handled by one provider in one reconcile; the provider observes the
  source kind already.
- **Out of scope, named:** cross-provider derivation (a directory fact
  materialising an object in another provider) needs ordering and source
  availability across providers; it is a follow-up decision.
- **Out of scope, named:** host materialisation (systemd units, containers). It
  needs a provider that runs on the target host; the rule shape is the same,
  the writer is a separate feature.

## Limits

| Limit | Reason |
|---|---|
| one source, one attribute | keeps the expansion a pure function and the plan explainable |
| list of strings only | no nested structures, no member-specific fields in v1 |
| no chaining | a derived resource cannot feed another rule; cycles and order are unsolved here |
| no filters | filtering is a property of the source declaration, not of the expansion |
| same provider only | cross-provider ordering and availability are a separate design |

## Consequences

- Providers are unchanged; the core expands and the plan grows.
- The derivation record becomes part of the reconciler state.
- Deletion budgets become a per-rule knob; the default freezes the set.
- The audit record can answer "why does this resource exist?" with a rule and a
  member.
- A rule bug is a configuration error (missing source, wrong attribute,
  duplicate id) or a bounded deletion (budget), never an unbounded action.

## Alternatives

| Alternative | Why not |
|---|---|
| external controller | duplicates ownership, deletion, retry, audit and health |
| NixOS rebuild per fact change | heavy; the generated configuration would live outside git |
| static declaration only | the use case needs an imperative script, which is the state we are leaving |
| unbounded derived deletions | a source bug would remove every materialised resource in one run |
