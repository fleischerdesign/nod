# ADR-044: A signing key is declared, not borrowed

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-043 (the OAuth2 contract), ADR-041 (one unit per stack).

## Context

An OAuth2 provider signs the tokens it issues. Authentik decides how from one
field, `signing_key`:

- **with** a keypair, tokens are signed with that key and the provider publishes
  a JWKS; clients verify RS256 by `kid`.
- **without** one, `OAuth2Provider.jwt_key` falls back to
  `(client_secret, HS256)` — no JWKS, and a client that verifies RS256 rejects
  every token.

The field references `crypto.CertificateKeyPair`. The default keypair is
*managed*: `CertificateKeyPairViewSet.queryset` excludes `managed=MANAGED_KEY`,
so `/api/v3/crypto/certificatekeypairs/` returns only the operator's own pairs
and a detail request for the internal one answers 404. Its primary key is
visible only as a reference from a provider that already uses it.

A provider created by nod without `signing_key` would therefore be a different
object than the one the blueprint created: same name, same client, same
redirects, HS256 instead of RS256. "A fresh install converges to the running
state" is the point of the declarative surface, so this is a defect, not a
detail.

## Decision

### 1. `authentik.certificatekeypair` is a kind

The id is the name, which is unique in Authentik. The material is input, not
state:

| Attribute | Kind | Notes |
|---|---|---|
| `certificate_path` | write-only | file holding the PEM certificate |
| `private_key_path` | write-only | file holding the PEM private key |

Both are required on create and never read on any other operation. They are
declared but not compared (ADR-043's `WRITE_ONLY` rule): an observation cannot
echo them, so comparing them would plan a no-op update every cycle.

The carrier is a **file**, not a variable. A PEM is a multi-line document and an
`EnvironmentFile` is line-oriented — a variable would be torn apart at the first
newline. Single-line credentials keep travelling as variables (`token_env`,
`client_secret_env`) because that is what they are.

The observation reports what the API exposes and the declaration never sets:
`key_type` and `fingerprint_sha256`. Those are facts for `provider status`, not
inputs.

### 2. Key material is immutable

`update` is refused: the API marks `certificate_data` and `key_data` write-only,
so a provider cannot read what it would replace, and silently replacing a
signing key breaks every client that cached a `kid`.

Rotation is the honest sequence, and each step is a normal plan entry:

1. declare a **new** keypair (a new name),
2. point the providers at it (`signing_key` changes, clients refetch the JWKS
   when the `kid` is unknown),
3. retire the old keypair — an explicit deletion, subject to the core's policy.

The consequence is stated plainly because it is a trap: a changed certificate
under an unchanged name is invisible to the provider. A new name is the only
rotation this kind understands.

### 3. `signing_key` is a name

`authentik.provider.oauth2` gains `signing_key`, resolved against the visible
keypairs — the same shape as flows by slug and scopes by managed key. An
unresolvable name is a configuration error, never a guess.

The observation maps the provider's `signing_key` pk back to a name. A pk that
names no visible keypair (the managed internal certificate) is omitted rather
than fabricated; a declaration that names a keypair then plans exactly one
update that moves the provider onto it.

### 4. The keypair is created before the provider

`CREATE_ORDER` starts with `authentik.certificatekeypair`; the delete order ends
with it. A provider that names a signing key in the same apply therefore finds
it, and a keypair is only removed after nothing references it.

## Consequences

- A fresh install produces the same providers as the running fleet: RS256 over
  JWKS, not HS256 over the client secret.
- The reconciler host holds a signing key. It already holds a superuser token,
  which can read every client secret and grant itself any audience; a signing
  key adds offline forgery to a set of powers that already includes online
  forgery. It is write-only like every other credential (ADR-043).
- The four existing OIDC providers move onto the declared key once. That is a
  visible change (new `kid`), so it is its own rollout, not a side effect.
- A test asserts that no observed attribute carries PEM material, and that an
  unknown `signing_key` fails the change instead of writing a null.

## Alternatives

| Alternative | Why not |
|---|---|
| leave `signing_key` null | HS256: no JWKS, and a strict client rejects the tokens |
| declare the keypair by primary key | a pk is per install; a declaration must survive a restore |
| discover the managed internal certificate | the API hides it (404, empty filtered list) |
| replace material in place on update | the API is write-only there, so nothing could be verified |
