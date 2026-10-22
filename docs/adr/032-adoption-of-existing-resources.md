# ADR-032: Adoption of existing resources

- **Status:** Accepted
- **Date:** 2026-10-22
- **Deciders:** maintainers
- **Refines:** ADR-030 §6 (ownership). Adds one optional provider operation.

## Context

Every new user of `nod` already has a world. A zone exists, created by hand,
by Terraform, by another GitOps tool, or by an earlier installation. That is
the normal case, not a migration edge.

`nod` only changes resources it owns: a provider with `marker` ownership marks
them, and everything else is foreign and left alone. That invariant is what
makes the tool safe — and it means that on a pre-existing world `nod` can
observe, but cannot correct drift. `planned 0` then means "the declaration
matches", not "the fleet owns this and will keep it matching".

Teaching a provider the comment vocabulary of some other tool would import that
tool's history into the contract. It would also only work for that tool. The
ownership transfer has to be a general mechanism, decided by the operator and
enforced by the core.

## Decision

### 1. Adoption is a first-class operation

`nod provider adopt` stamps the provider's ownership marker on existing
resources that already belong to the declaration. It is the only path that
changes ownership, and it is explicit: a normal reconcile never adopts, even
when every attribute matches.

### 2. The core decides eligibility

For every observed resource, in this order:

| Observation | Decision |
|---|---|
| already carries the marker | `already-owned`, nothing to do |
| declared, attributes match | `eligible`, may be stamped |
| declared, attributes differ | `conflict`, stamped only with `--force <id>` |
| not declared | `undeclared`, never touched |

A difference is reported as `key: have != want`, limited to the keys the
declaration names. Bookkeeping fields a provider adds (`record_id`, its own
marker) are not desired state and never make a conflict.

An `--force` id that names nothing declared is a configuration error, not a
silent no-op.

### 3. The provider writes only its marker

The `adopt` request carries the observed resources; the provider sets its own
marker and touches nothing else — not content, not TTL, not proxy state. It
does not interpret comments, labels or names. Requiring `record_id` and
failing per resource is a provider's business; the core reports the failure.

### 4. `unowned` is part of the plan

The reconcile report gains `unowned`: declared resources that exist and match
but do not carry the marker. It is the onboarding signal. `0` means the zone is
correct and owned; anything else means drift correction is not active for those
records yet.

### 5. The operation is optional and additive

The handshake and the manifest grow `operations` (`["adopt"]`). An old core
ignores it, an old provider omits it, no protocol major changes. A core that
needs adoption refuses a provider that does not declare it, and `provider test`
calls `adopt` with an empty list to prove the operation exists and writes
nothing.

### 6. Ownership modes decide whether adoption applies

| Mode | Adoption |
|---|---|
| `marker` | yes, this ADR |
| `namespace` | not needed: the namespace is the ownership statement |
| `none` | impossible: read-only provider |

A provider without a marker field (a zone API that has no comments) uses
`namespace` or stays read-only. The core never assumes a comment exists.

## Consequences

- A new user's path is: declare → reconcile (`unowned N`) → `adopt --apply` →
  reconcile (`unowned 0`). Drift correction starts after the adoption, not
  before.
- A conflicting record is visible before anything is written, and taking it
  over is a per-id decision. Onboarding an existing zone does not require
  deleting and recreating records, so there is no DNS window and no broken
  ACME challenge.
- The marker is the only field adoption writes. An operator can see exactly
  what changed, and a provider's audit trail shows a comment update, not a
  content change.
- Adoption is not reversible in one command: it transfers ownership, it does
  not record an undo. Reverting means changing the declaration, not rolling
  back a plan.

## Rejected alternatives

- **Teach providers legacy comment prefixes.** Imports one tool's history into
  the contract and helps exactly one migration.
- **Adopt automatically when attributes match.** Makes ownership a guess and
  turns a read-only reconcile into a write.
- **Delete and recreate the records.** A DNS window, a broken ACME renewal and
  a risk of different proxy/TTL settings, for a change that only needs a
  marker.
- **A separate migration tool outside `nod`.** A second implementation of the
  same rules, with its own bugs and no conformance test.

## Open questions

- Should a later `nod provider release` drop the marker again (the inverse of
  adoption) for handing a zone back?
- Should `unowned` be part of the exit status, so a pipeline can gate on
  "declared but not owned"?
