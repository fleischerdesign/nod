# ADR-043: OAuth2 providers are write-only about their secret

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-030 (provider protocol), ADR-038 (authority), ADR-041 (the
  proxy stack is one unit).

## Context

Authentik's proxy providers are an `OAuth2Provider` subclass, and the API does
not hide it in the way one would expect:

- `/api/v3/providers/oauth2/` uses one fixed serializer, so it returns **every**
  provider — the 18 proxy rows included — each labelled `oauth2provider`.
- `/api/v3/providers/all/` uses `select_subclasses()` and reports the real model
  (`authentik_providers_proxy.proxyprovider` vs
  `authentik_providers_oauth2.oauth2provider`). It is the only honest type
  source.
- The OAuth2 serializer returns `client_secret` in cleartext.

A naive OAuth2 kind would therefore manage proxy providers as OAuth2 providers,
and would copy a live secret into observation output. Both are unacceptable.

## Decision

### 1. Type by the honest list, not by the typed endpoint

`oauth2_providers()` reads `/providers/all/` into a `pk → meta_model_name` map
and keeps only rows of the OAuth2 endpoint whose pk is typed
`authentik_providers_oauth2.oauth2provider`. No subtraction, no serializer
trust; the filter is positive and testable.

### 2. The secret is write-only and optional

`client_secret_env` names an environment variable, never a value.

- **create** requires it; without it the change fails loudly.
- **update** and **adopt** never read it; the existing secret stays untouched.
- **observe** strips `client_secret`; it appears in no attribute, plan, report
  or audit entry.

The migration of existing providers therefore needs **no** secret on the
reconciler host. Only a genuinely new OAuth2 provider needs a source, and then
the variable is declared next to the token variable it already uses.

### 3. Names, not identifiers

The declaration names what a human knows: flows by slug, the signing key by
name, redirect URIs as strings (mapped to `{matching_mode: "strict", url}`),
scope mappings by their `managed` value (managed scopes) or by name (custom
ones). The provider resolves them to primary keys; an unknown name is a
configuration error.

### 4. Provider and application move together

ADR-041 applies unchanged: a provider is created before the application that
names it and deleted after it. An OAuth2 provider has no outpost dependency, but
the reference is the same, so the ordering is the same.

## Consequences

- The last blueprint-managed stack can move to the reconciler without moving a
  single secret.
- A new OAuth2 provider needs one environment variable, named in the
  declaration; nothing else changes.
- A test asserts that no observed attribute contains `client_secret`, and that
  an update succeeds without `client_secret_env`.
- The type filter is a provider-internal detail; the core sees one kind,
  `authentik.provider.oauth2`.

## Alternatives

| Alternative | Why not |
|---|---|
| subtract the proxy pk set | works, but encodes a negative guess instead of reading the type |
| `client_secret` in the declaration | the secret would land in the Nix store and every report |
| `client_secret_file` | a second secret mechanism beside `token_env`; rotation is harder to see |
| keep the OIDC stack in the blueprint | leaves the last stack with two writers and no ownership record |
