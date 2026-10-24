# ADR-035: The provider owns Authentik

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-030 (provider protocol), ADR-032 (adoption), ADR-034
  (declared deletions). Retires the blueprint renderer as a writer.

## Context

Authentik is declared today in two languages. The provider owns
`authentik.group`; blueprints own users, roles, providers, applications,
outposts, flows, stages, bindings, policies, brands, sources and certificates.
That split is not a stable end state:

- **Two writers, one graph.** A blueprint that references a provider-owned group
  by `!Find` needs the group to exist when the blueprint applies. On a running
  fleet it does; on a fresh database it does not, and the apply fails loudly
  after the fact (`relation_diffs` counts the missing bindings). The group
  cutover exposed exactly this: the blueprint graph cannot depend on a writer
  that runs later.
- **The declaration is split.** A change that touches a group and an
  application is reviewed in two languages and applied by two mechanisms.
- **Ownership is split.** Some objects carry the fleet marker, others are
  "blueprint-owned" by convention. There is no single answer to "does the fleet
  own this?".

The target is one writer: the `authentik` provider owns every object this
repository declares, and Authentik's own bootstrap keeps only what an external
client cannot create for itself.

## Decision

### 1. One writer per object

Every object in `my.contracts`/`my.directory` that this repository declares is
owned by the provider. Upstream objects (the `default-*` flows, stages and
brands Authentik creates itself) are referenced, never owned.

The only blueprint that remains is the provider's own credential
(`vyrx-provider-access`): a client cannot create the token it authenticates
with. That blueprint declares a user, a group, a role and a token — no part of
the application graph.

### 2. The core stays agnostic

The provider speaks Authentik's HTTP API; the core knows nothing about
Authentik. Every new kind is a `capabilities` entry, a resource model and an
API client inside the provider crate. The core keeps owning the protocol,
ownership policy, deletion policy and the plan record.

### 3. The plan is dependency-ordered

The core applies changes in the order the provider returns them. The provider
therefore plans topologically, and the order is part of its contract:

```
users → groups → roles → flows/stages/stage-bindings
      → providers/applications/policy-bindings → outposts → brands/sources/certificates
```

A create whose identifier another create references must come first. The plan
is still approved or refused as a whole; ordering is not a substitute for the
policy.

### 4. References by stable id, pks from the create response

Desired resources address each other by stable id (username, group name, slug,
name). The provider resolves them against the API and, for objects it creates
in the same run, keeps the primary key the create response returned. A plan
that creates a group and its members resolves the members after creating them,
not from a snapshot taken before the run.

### 5. Ownership is per kind

| Kind | Marker |
|---|---|
| objects with `attributes` (user, group, provider, application, source, …) | `attributes.<marker_key> = marker_value` |
| objects with a free-form config (outpost) | the same marker inside `config` |
| objects without a free-form field (policy binding, stage binding) | derived: a binding is owned because its target is owned |
| the provider's own credential | the bootstrap blueprint, never adopted |

Derived ownership is deliberate: a nameless relation has no place for a marker,
and inventing one (description prefixes, hidden fields) would make the
declaration depend on a spelling. A binding follows its target.

### 6. Replacement is explicit

Some changes are not updates: a provider type cannot be changed in place, a
certificate cannot be re-keyed. The provider may need delete-then-create for the
same id. That is a **replace**, not an implicit deletion: it is declared, it is
visible in the plan, and it is exempt from the deletion cap for the same reason
a declared deletion is (ADR-034). The core gains a `replace` change kind; an
implicit delete stays what the cap guards.

### 7. A fresh database converges in one activation

The order is:

1. Authentik starts and applies its own bootstrap (upstream defaults).
2. The credential blueprint applies (`vyrx-provider-access`).
3. The provider runs once against the fresh API and creates the whole declared
   graph in dependency order.
4. Nothing else applies — there is no second blueprint layer to order.

A fresh host needs no second pass, no hourly discovery retry and no manual
adoption of objects the blueprint created.

### 8. The migration is a dependency order, not a phase plan

Kinds become provider-owned in the order their dependents need them:

| Step | Kind | Why here |
|---|---|---|
| 1 | `authentik.user`, `authentik.group` | the directory every other object references |
| 2 | `authentik_rbac.role`, object permissions | groups and bindings reference roles |
| 3 | flows, stages, stage bindings | providers reference flows |
| 4 | providers, applications, policy bindings | outposts and audiences reference them |
| 5 | outposts | depend on providers, applications and roles |
| 6 | brands, sources, certificates | leaves; no one references them |

Each step removes its objects from the blueprint renderer, adds the kind to the
provider's capabilities and to the fresh-database check. The blueprint set
shrinks until only the credential is left.

## Consequences

- The directory blueprint (`01-rbac/users.yaml`) disappears; `enrollment.yaml`
  and the LDAP blueprints lose their `metaapplyblueprint` dependency on it.
- The apply unit splits into a bootstrap phase (credential), a provider phase
  (the graph) and a verification phase. `nod provider apply` becomes a core
  command so the host can run one provider without a daemon.
- The fresh-database check must apply the whole remaining blueprint set, not one
  file, so a dependency on a provider-owned object fails in CI rather than on a
  restore.
- Adoption is needed once per existing kind (ADR-032); a created mis-ownership
  is a bug, not a routine step.
- The provider grows; the core does not.
