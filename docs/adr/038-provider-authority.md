# ADR-038: Provider authority

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-030 (provider protocol), ADR-035 (ownership per kind),
  ADR-037 (ownership by evidence).

## Context

A provider acts as one account: the protocol has no per-object credential, and
the reconciler is the only writer for the objects it owns. Two things follow.

First, the account must be able to see and write everything the provider
manages. A permission list that covers today's kinds becomes a trap the moment
a kind is added.

Second, an API may return less than the truth without saying so. Authentik's
`/core/applications/` filters by policy for every non-superuser: the paginator
counts the whole queryset, the response contains only what the account may
access, and `superuser_full_list=true` is honoured only for a superuser.
Measured: a service account holding the eleven object permissions saw **2 of
22** applications. A declared application outside that set would be planned as
`create` on every run, fail with 409 and never verify — a silent subset, not a
smaller world.

## Decision

### 1. One grant, full visibility

A provider account receives the authority the provider needs, not a list of
individual permissions. For Authentik that is membership in a group with
`is_superuser = true`: the model derives `User.is_superuser` from group
membership (`core/models.py`), and the user serializer exposes the field
read-only, so a blueprint must grant it through the group.

The fleet's provider token is host-local in SOPS, the account cannot log in
interactively, and no other host holds it. The blast radius is the identity
provider, and that is the intended boundary: the reconciler is its only writer.

### 2. The provider proves completeness

A provider that lists objects must detect a filtered view and refuse it. The
Authentik provider compares the paginator's total with the number of objects it
received and fails the observation with a config error naming the requirement.
Degrading to a condition would be wrong: a condition leaves the plan running
against a partial observation.

A missing permission remains a condition where the provider can still do useful
work (`view_user`, `view_application`), so `nod provider status` shows the gap
before the first write. A *filtered* list is never degraded.

### 3. The contract lives with the provider

The handshake's `required_permissions` and the provider's README say what the
provider touches. The fleet grants the account; it does not duplicate the list
as a role. One grant, one documentation site, no drift between two lists.

### 4. Least privilege is a property of the API

Where an API returns the complete set to a narrow account, least privilege stays
possible. Where it cannot, the account is broad and the provider says so. A
provider never pretends to be narrow while seeing a subset.

## Consequences

- A misconfigured token fails on the first observation instead of managing a
  subset; the message names what is wrong.
- Adding a kind whose list is policy-filtered needs no new permission model: the
  same completeness check covers it.
- The account's authority is one fact (group membership), not a permission list
  that can silently fall behind the code.
- A provider for an API without a total count cannot make this check. It must
  then either document that its list is complete by construction, or pass the
  declarations it needs to `observe` — the next point.

## Out of scope, named

Passing the declared and recorded ids to `observe` would let a provider fetch
exactly the objects it owns and is asked for, without a full-visibility account.
That is the clean long-term answer for APIs that filter by policy, but it is a
protocol change for every provider and a change to what "observed" means. It is
deferred; this ADR keeps the current contract honest in the meantime.

## Alternatives

| Alternative | Why not |
|---|---|
| a role per kind | sees a subset, plans creates for invisible objects, fails silently |
| a policy binding per application | per-object maintenance; a new application stays invisible until someone binds it |
| trusting the list without a completeness check | the failure this ADR exists to prevent |
| making every provider's account a superuser | wrong outside Authentik; authority is per API, not a fleet rule |
