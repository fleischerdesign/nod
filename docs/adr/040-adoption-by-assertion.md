# ADR-040: Adoption by assertion

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-032 (adoption), ADR-037 (ownership by evidence),
  ADR-038 (provider authority).

## Context

ADR-032 adoption stamps the provider's marker on an existing object. ADR-037
then said a marker-less kind is created by the fleet, not adopted after the
fact — because there is no field to write. That leaves a real migration without
a path: an application created by a blueprint exists, has no marker, and cannot
be taken over. The only alternative was to delete it and let the reconciler
create it again, which means a window in which the application is gone.

An operator can say what a file cannot: "this object is ours". That statement is
evidence of the same kind the marker is — a fact the fleet holds — and it can be
recorded.

## Decision

### 1. Adoption records evidence

`nod provider adopt --apply` writes an `adopted` entry into the provider's
ownership record (`providers/<provider>.ownership.json`) for every resource the
provider confirmed. The marker, where the kind has one, is still stamped; the
record makes the two forms one model.

### 2. A marker-less kind is verified, not written

For such a kind the provider writes nothing. It checks that the object exists
and reports `ok` with the message that ownership is the core's to record. The
core has already decided eligibility from the declaration and the provider's own
plan, so the assertion is bounded: only declared, satisfied resources are
eligible, and a conflicting one needs `--force <id>`.

### 3. Adoption stays explicit

Nothing adopts implicitly. A declaration never claims an existing object; a
reconcile never does either. `adopt` is an operator command, and its effect is
audited like a reconcile.

### 4. The record is pruned like a creation

An `adopted` entry disappears when the provider no longer reports the object, so
a deleted application does not stay owned forever. A lost record makes the
resource foreign again: reported, never modified.

### 5. The command holds the provider lock

`adopt` runs under the same `ProviderLock` as the daemon and writes the record
before releasing it, so a concurrent reconcile cannot rewrite the record from an
older snapshot. An ad-hoc `--command` without a configured provider name cannot
take that lock; the limitation is documented.

## Consequences

- A blueprint-managed application migrates without a window: declare it, adopt
  it, remove the blueprint's application entry.
- The operator's statement is the proof; the record is load-bearing. Losing it
  degrades to "foreign and reported", never to a silent claim.
- The provider's `adopt` operation now covers kinds it cannot mark, so the
  handshake's `ownership: "marker"` remains true at the provider level without
  being the only evidence.
- Two writers on one object are avoided: the blueprint keeps the provider, the
  reconciler owns the application and its audience (ADR-039).

## Alternatives

| Alternative | Why not |
|---|---|
| delete and recreate | a window without the application, and a manual step at the worst time |
| hand-editing the ownership record | an unsupported interface that the next reconcile may prune |
| a marker in a visible field | rejected in ADR-037 |
| claiming any declared object implicitly | turns migration into a surprise takeover |
