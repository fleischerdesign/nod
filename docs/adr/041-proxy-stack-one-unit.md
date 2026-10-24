# ADR-041: The proxy stack is one unit

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-038 (provider authority), ADR-039 (application audience),
  ADR-040 (adoption by assertion).

## Context

Authentik serves a proxy provider only when an outpost lists it. The embedded
outpost carries that list (`providers`), and the list has exactly **one** writer:
the API replaces it as a whole. The fleet's applications, their providers and
the outpost were all declared by the same blueprint, so the ordering was free.

Splitting them across two writers breaks that in both directions:

- The outpost moves to the reconciler, the providers stay in the blueprint: the
  reconciler resolves provider names the blueprint has not created yet on a
  fresh deploy, and the outpost update fails or omits the new provider.
- The providers move, the outpost stays: the blueprint references a provider
  that the reconciler creates later on another host, with the same result.

Either way, a new application's forward authentication is broken until someone
applies a second time. That is not a transient risk to accept; it is a
structural one.

## Decision

### 1. One declaration set, one writer

Providers, the outpost and applications are declared together and written by
one actor. The fleet moves them to the reconciler as a set; the blueprint hands
the whole proxy stack over, not a part of it.

### 2. The plan orders the dependency

Creates and updates run provider → outpost → application; deletes run the
reverse, application → outpost → provider. A provider still referenced by an
application that is not deleted in the same plan is refused, not silently
detached.

### 3. The embedded outpost is never deleted

Authentik recreates it on startup (`managed =
goauthentik.io/outposts/embedded`). A delete change for it is refused with that
explanation; removing its providers is the supported way to stop serving
something.

### 4. The outpost's provider list comes from the declarations

The list names the providers of the declared publications, not every provider
the API reports. Retired endpoints leave orphaned providers behind — the
measured fleet had seven — and they must not be attached to the outpost again by
a projection that reads the observation.

## Consequences

- A publication's provider cannot migrate alone; the migration is the proxy
  stack, which is why the whole set is adopted before the blueprint lets go.
- A new endpoint is served in the same reconcile that creates it: provider,
  outpost, application, in that order.
- The blueprint keeps only what is not part of the stack (flows, the OIDC
  stack until it is modelled, LDAP outposts).
- The outpost's `config` is merged on update, so a key the declaration does not
  name keeps its value.

## Alternatives

| Alternative | Why not |
|---|---|
| two writers on the outpost's list | the API replaces the list; the writers clobber each other |
| per-provider outpost bindings as separate resources | the list is still one API field |
| tolerate the gap until the next apply | forward authentication fails silently for a new application |
| read the outpost's providers from the observation | attaches retired providers; the measured fleet has seven |
