# ADR-046: Built-in scope mappings are not resources

- **Status:** Accepted
- **Date:** 2026-10-25
- **Deciders:** maintainers
- **Refines:** ADR-043 (the OAuth2 contract), ADR-044 (declared artifacts).

## Context

An OAuth2 provider's `property_mappings` is a list of `ScopeMapping` foreign
keys. The endpoint that lists them, `/api/v3/propertymappings/provider/scope/`,
returns **two populations**:

- Authentik's own mappings, which carry a `managed` key such as
  `goauthentik.io/providers/oauth2/scope-openid`. They exist in every
  installation, and every provider uses them.
- Mappings an operator created — the ones an endpoint adds beside the standard
  scopes (`username`, `groups`, a tenant id).

The OIDC blueprint compiled the second population. Moving the provider stack to
the reconciler left that compiler as the last piece of the blueprint for
applications, and a `checks` entry would not have closed the gap: the mapping is
an object, not an assertion.

## Decision

### 1. `authentik.scope_mapping` covers the custom population only

The id is the name. The attributes are `scope_name`, `expression` and
`description`; nothing is write-only, because a mapping has no secret.

The observation **skips** a mapping whose `managed` key is set. A built-in is not
a resource: it exists in every installation, no declaration creates it, and
reporting it as unowned would inflate the status with objects nobody may touch.
A declaration that names one fails at create with the API's own conflict — the
message names the object, and the fix is to reference it by scope key.

### 2. The provider references a mapping the way it always did

`property_mappings` lists scope keys. A managed key (`openid`, `email`,
`profile`) resolves to the built-in; any other name resolves to a custom mapping,
declared or adopted. The declaration therefore does not change when a mapping
moves from "blueprint-created" to "declared here".

### 3. The mapping is created before the provider that names it

`CREATE_ORDER` places `authentik.scope_mapping` before the providers; the delete
order ends with it. A provider created in the same apply finds its mapping.

### 4. The blueprint keeps only the policy

With mappings and providers declared, `vyrx-apps-oidc` contains the
`accessAuthenticated` policy and the retirement tombstones — the one audience a
group cannot express. Everything else about an application is the reconciler's.

## Consequences

- The OIDC blueprint is reduced to policy and retirement: one writer for the
  provider stack, exactly as ADR-041 intended.
- A custom mapping is a first-class resource: planned, verified, and owned by the
  record like every other marker-less kind.
- Authentik's built-ins stay invisible to the plan. They are addressed by scope
  key, never by name, and no run can delete them.

## Alternatives

| Alternative | Why not |
|---|---|
| keep the blueprint compiler for mappings | the last two-writer corner, for an object a kind models in forty lines |
| treat built-ins as unowned resources | status noise, and a declaration could plan a delete of an object every provider needs |
| map by human name only | the name is translatable text; the managed key is the stable identifier |
