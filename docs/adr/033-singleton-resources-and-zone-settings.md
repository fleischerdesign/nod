# ADR-033: Singleton resources and zone settings

- **Status:** Accepted
- **Date:** 2026-10-22
- **Deciders:** maintainers
- **Refines:** ADR-030 §4–6 (operations, policy, ownership). Complements ADR-032 (adoption).

## Context

Not every thing a provider manages is a list. A zone has exactly one set of edge
settings: TLS mode, Always Use HTTPS, minimum TLS version. There is no second
instance, nothing to create, nothing to delete, and no comment to carry a
marker. The declaration already names these values; before this ADR a separate
tool reconciled them, which meant two engines and a second source of truth for
one zone.

The protocol's resource model is a list of `{kind, id, attributes, owned}`.
Singletons fit the shape but not the lifecycle: `create` and `delete` are
meaningless, and `owned` cannot come from a marker.

## Decision

### 1. A provider may expose singleton resources

A singleton is a resource with exactly one id inside a provider instance, for
example `zone.settings`. It is always observed and never enumerated.

### 2. A singleton is owned by construction

`owned = true` means "the declaration is the only writer". The marker applies
to enumerable resources, where a foreign instance can exist; a zone's settings
have no instances to be foreign. A provider that shares a singleton with
another tool must not expose it, and a user must not declare one they do not
own.

### 3. Absence means "do not change"

If the declaration does not name the singleton, the provider proposes nothing.
It never resets the settings to defaults and never deletes them. A reset is a
change like any other and must be declared.

### 4. Only `update` exists

A singleton plan contains `update` when a declared key differs, and nothing
otherwise. `create` and `delete` are invalid for a singleton; the core's
policy still applies — an update is allowed only when `owned` is true, which
for a singleton it is by construction.

### 5. Kinds are neutral

The kind is `zone.settings`, not `cloudflare.zone_settings`. Any provider that
manages an edge zone can implement it; the attributes are the domain's, not the
vendor's. Keys the declaration omits are left alone.

### 6. The provider validates, the core plans

The provider checks that `ssl`, `always_use_https` and `min_tls_version` carry
values its API accepts, and reports a `config` error otherwise. The core does
not know the vocabulary of a singleton; it compares declared keys and applies
the update.

## Consequences

- One engine owns the zone: records (marker, adoption) and settings
  (singleton) are the same reconcile loop.
- The old settings reconciler disappears with its timer, app and check; the
  declaration is the only source.
- A zone whose settings another tool manages must not be declared here. That
  is a documentation obligation, not something the core can detect.
- Adoption (ADR-032) does not apply to singletons: there is nothing to stamp.
  A singleton is either declared or not.

## Rejected alternatives

- **Keep a separate settings tool.** Two engines, two sources of truth, and a
  second reconciliation cadence for one object.
- **Model the settings with `namespace` ownership.** It would make the whole
  zone the namespace and let a plan delete records; the marker already covers
  those.
- **Reset to defaults when the declaration omits the singleton.** Silence would
  become a destructive instruction.
- **`create`/`delete` for a singleton.** It has no lifecycle; the words would
  be lies in the plan.

## Open questions

- Should a provider be able to expose a singleton as read-only (observed, never
  updated) for audit-only integrations?
- Should the plan report the old and new value per key, as adoption reports the
  diff? The current report names the resource, not each field.
