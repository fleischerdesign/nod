# nod-provider-authentik

Authentik groups, users and applications for [nod](https://github.com/fleischerdesign/nod),
spoken over the provider protocol (JSON Lines on stdin/stdout).

The provider owns `authentik.group`, `authentik.user`, `authentik.application`,
`authentik.provider.proxy`, `authentik.provider.oauth2`,
`authentik.certificatekeypair` and `authentik.outpost` objects: it observes
them, plans the difference against a declaration, applies it, and takes
ownership of existing objects on request. Roles, parents and policy bindings
stay outside its scope.

## Capabilities

| Kind | Id | Attributes |
|---|---|---|
| `authentik.group` | group name (unique in Authentik) | `is_superuser`, `description`, `users` |
| `authentik.user` | username | `name`, `email` |
| `authentik.application` | slug (unique in Authentik) | `name`, `provider`, `meta_launch_url`, `meta_description`, `meta_publisher`, `open_in_new_tab`, `policy_engine_mode`, `group`, `meta_hide`, `access_groups` |
| `authentik.provider.proxy` | provider name (unique in Authentik) | `mode`, `external_host`, `cookie_domain`, `authorization_flow`, `invalidation_flow` |
| `authentik.provider.oauth2` | provider name (unique in Authentik) | `client_id`, `authorization_flow`, `invalidation_flow`, `redirect_uris`, `sub_mode`, `include_claims_in_id_token`, `grant_types`, `access_token_validity`, `property_mappings`, `signing_key`, `client_secret_env` (write-only) |
| `authentik.certificatekeypair` | keypair name (unique in Authentik) | `certificate_path`, `private_key_path` (both write-only) |
| `authentik.scope_mapping` | mapping name | `scope_name`, `expression`, `description` (custom mappings only; built-ins are addressed by scope key) |
| `authentik.outpost` | outpost name | `type`, `providers`, `config.authentik_host`, `config.authentik_host_browser`, `config.authentik_host_insecure` |

`users` lists login names. `description` and `users` are written into the group's
free-form `attributes` and membership respectively; `is_superuser` is the group's
own flag.

`provider` names a provider the way a human knows it (for example
`grafana-oauth`); the provider resolves it to the primary key the API expects
and fails the change when the name is unknown. An empty string detaches it.
`meta_icon` and the backchannel providers are not managed: the first is a file
upload, the second a relation this provider does not own.

`access_groups` names the groups that may launch the application. The declared
list is the complete set of **group** bindings: a group binding outside it is
removed, while user and policy bindings are left alone. An absent list leaves
the bindings untouched; an empty list removes every group binding, which makes
the application fall back to authentik's `core_default_app_access` — open to
every authenticated user unless another binding restricts it. Grants are made
before removals, so a failure never cuts access; a create whose bindings fail
is deleted again rather than left open.

`authorization_flow` and `invalidation_flow` name flows by **slug**; the
provider resolves them to the uuids the API expects. A proxy provider is
created before the application that names it and deleted after it; a provider
still referenced by an application that is not deleted in the same plan is
refused instead of silently detaching it.

### OAuth2 providers

`client_id` is public and part of the observed resource. The **client secret is
not**: it is write-only, it has no place in a resource, a plan or a report, and
the provider's API types do not even carry the field (ADR-043). A `create`
reads it from the environment variable named by `client_secret_env` — a
variable, never a value — and an `update` never sends the key, so an adopted
provider keeps the secret it already has. That is what makes migrating an
existing OIDC provider safe: nod takes ownership without touching the
credential.

`property_mappings` lists **scope keys** (`openid`, `email`, `profile`, …), not
uuids. The provider resolves them against the managed scope mappings; an
unknown key is a declaration error. `redirect_uris` is the complete list of
registered URLs and is written with `matching_mode: strict`.

`signing_key` names a keypair (below). Without one, Authentik signs with HS256
and the client secret — no JWKS, and a strict client rejects the token. An
unknown name is a configuration error, never a silent null (ADR-044).

`client_secret_env` is an **input, not state**: it names the environment
variable that holds the secret for a `create`. It is declared, never observed
and never compared — otherwise every cycle would plan an update that changes
nothing. Rotating a secret is not a reconcile action: nod neither reads nor
rewrites the secret of an existing provider.

### Certificate keypairs

A keypair signs tokens. Its material is a PEM **document**, so the declaration
names files (`certificate_path`, `private_key_path`), not variables: an
`EnvironmentFile` is line-oriented and would tear a PEM apart at the first
newline.

Both paths are inputs, declared but never compared. The observation reports
only facts the API exposes — `key_type`, `fingerprint_sha256` — and never the
material; the API marks `certificate_data` and `key_data` write-only in any
case.

A keypair is **immutable**: `update` is refused, because a provider that cannot
read what it replaces must not rewrite it, and a silently replaced signing key
breaks every client that cached a `kid`. Rotation is a new name, providers
moved onto it, old pair retired — three normal plan entries. A changed
certificate under an unchanged name is invisible: declare a new one.

Authentik's own managed JWT certificate cannot be named through the API
(`/crypto/certificatekeypairs/` excludes managed rows), which is exactly why a
fleet that wants a deterministic signing key declares its own.

A fresh installation produces the material once, outside the declaration:

```bash
openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 3650 \
  -subj "/CN=nod OIDC signing key/O=nod" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -addext "keyUsage=critical,digitalSignature" \
  -keyout key.pem -out cert.pem
```

Keep both PEMs in the secret store that feeds the reconciler host's filesystem
(SOPS in this fleet), declare the two paths, and the first apply creates the
keypair. Nothing in the repository carries the key.

A declared OAuth2 provider is verified against the honest type map from
`/providers/all/`: the typed endpoint serializes a proxy provider as an OAuth2
provider too, so only `meta_model_name` tells them apart. A proxy row is never
planned as an OAuth2 provider, and an OAuth2 provider is never mistaken for one
of its subclasses.

### Scope mappings

An OAuth2 provider's `property_mappings` lists scope keys. A key Authentik ships
(`openid`, `email`, `profile`) resolves to its built-in mapping; any other name
resolves to a custom mapping, which this provider can declare.

The built-ins are **not resources**: they carry a `managed` key, exist in every
installation, and no run may touch them. The observation skips them, so a plan
never reports them as unowned and no declaration can plan their deletion. A
declaration that tries to create one fails with the API's conflict — the fix is
to reference it by scope key (ADR-046).

### Outposts

An outpost is observed and can be taken over, but not fully managed: its
`config` object carries connection and TLS settings the provider only reports.
`providers` is the list of provider **names**; authentik only serves a provider
that an outpost lists, which is why the relation is visible here.
`private_key`, `token` and the docker/kubernetes settings are deliberately
absent from the resource.

## Settings

```json
{
  "url": "https://auth.example.com",
  "token_env": "AUTHENTIK_TOKEN",
  "marker_key": "nod",
  "marker_value": "owned",
  "page_size": 100,
  "timeout_secs": 30
}
```

Only `url` is required. The token is read from the environment variable named by
`token_env`; it never appears in argv, a plan or a resource.

The ownership marker lives in the group's `attributes` object:
`attributes.{marker_key} == marker_value`. Authentik groups have no `labels`
field, so `attributes` is the portable place for it. The marker is
**configuration**: this fleet uses `vyrx=owned`, another installation uses
whatever it declares.

Applications have no free-form field — only `meta_*`, `group` and `provider`,
all of them user-visible. They therefore never carry a marker: the provider
reports `owned: false` and the core owns them through its record (ADR-037).
`adopt` refuses an application for the same reason.

## Desired state

```json
{
  "resources": [
    {
      "kind": "authentik.group",
      "id": "media",
      "attributes": {
        "is_superuser": false,
        "description": "Media consumers",
        "users": ["philipp"]
      }
    }
  ]
}
```

A missing resource is planned as `create`; an owned resource whose declared
attributes differ is `update`; an owned resource outside the declaration is
`delete`. Foreign resources are never updated or deleted: adoption is an
explicit operation, not a reconcile side effect.

### Only declared attributes are managed

The declaration is a subset, not a replacement. A key the resource omits is
neither compared nor written:

- `users` absent means membership stays UI-owned. The group keeps its members.
- `users: []` is a declaration and removes every member.
- Any other attribute outside the declaration survives an update untouched.

This is what lets a fleet declare a group's purpose without taking ownership of
a membership that an LDAP sync or an administrator maintains.

## Operations

| Operation | Effect |
|---|---|
| `observe` | Reads every group, user, application, provider and outpost, paginated. |
| `plan` | Computes `create`/`update`/`delete` against the declaration. |
| `apply` | Writes the plan. Refuses every change to an unowned resource. |
| `adopt` | Takes ownership on request: a group or user is stamped with the marker, a provider or outpost is verified to exist, and the core records the decision (ADR-040). An application is refused. |

Deletion is the core's decision, not the provider's: the core's `policy.protect`
and `policy.max_deletions` bound what a plan may remove, and a plan over the cap
is refused as a whole.

## Permissions

The manifest and the handshake name what the token must be allowed to do:

| Permission | Needed for |
|---|---|
| `authentik_core.view_group` | `observe`, `plan`, `apply` |
| `authentik_core.view_user` | resolving declared members to primary keys |
| `authentik_core.view_application` | `observe`, `plan`, `apply` |
| `authentik_core.view_provider` | resolving a declared provider name to its pk |
| `authentik_core.change_group` | `update`, `adopt` |
| `authentik_core.add_group` | `create` |
| `authentik_core.add_user` | `create` of a user |
| `authentik_core.change_user` | `adopt` of a user (the marker) |
| `authentik_core.add_application` | `create` of an application |
| `authentik_core.change_application` | `update` of an application |
| `authentik_core.delete_application` | `delete` of an application |
| `authentik_providers_proxy.view_proxyprovider` | `observe`, `plan`, `apply` of a proxy provider |
| `authentik_providers_proxy.add_proxyprovider` | `create` of a proxy provider |
| `authentik_providers_proxy.change_proxyprovider` | `update` of a proxy provider |
| `authentik_providers_proxy.delete_proxyprovider` | `delete` of a proxy provider |
| `authentik_providers_oauth2.view_oauth2provider` | `observe`, `plan`, `apply` of an OAuth2 provider |
| `authentik_providers_oauth2.view_scopemapping` | resolving a declared scope key to its uuid, and observing custom mappings |
| `authentik_providers_oauth2.add_scopemapping` | `create` of a custom mapping |
| `authentik_providers_oauth2.change_scopemapping` | `update` of a custom mapping |
| `authentik_providers_oauth2.delete_scopemapping` | `delete` of a custom mapping |
| `authentik_providers_oauth2.add_oauth2provider` | `create` of an OAuth2 provider |
| `authentik_providers_oauth2.change_oauth2provider` | `update` of an OAuth2 provider |
| `authentik_providers_oauth2.delete_oauth2provider` | `delete` of an OAuth2 provider |
| `authentik_outposts.view_outpost` | `observe`, `plan`, `apply` of an outpost |
| `authentik_crypto.view_certificatekeypair` | `observe`, `plan`, `apply` of a keypair |
| `authentik_crypto.add_certificatekeypair` | `create` of a keypair |
| `authentik_crypto.change_certificatekeypair` | reserved: a keypair is immutable, the permission is listed for completeness |
| `authentik_crypto.delete_certificatekeypair` | `delete` of a keypair |
| `authentik_flows.view_flow` | resolving a flow slug to its uuid |
| `authentik_core.delete_group` | `delete`, only when the core's policy allows it |

`observe` checks the read permissions by using them: a token without
`view_group` fails the observation with the permission named, and a token
without `view_user` still observes the rest and reports a condition, so
`nod provider status` shows the gap before the first write. The write
permissions cannot be probed without writing; a rejected write names the
permission it needed instead of saying "credentials rejected".

### Authority, not a permission list

Authentik filters the application list by policy for every non-superuser: the
paginator counts all objects, the response contains only those the account may
access. A narrow token therefore sees a subset, and a provider acting on it
would treat invisible applications as absent. Two consequences:

- The account needs full visibility — in this fleet a service account in a
group with `is_superuser = true`; authentik derives superuser status from group
membership, and the user serializer exposes the field read-only (ADR-038).
- The provider proves completeness: it compares `pagination.count` with the
number of applications it received and fails the observation with a config
error naming the requirement. A filtered list is never a smaller world.

The table above documents what the provider touches. A narrower account can
manage groups and users; applications need the full view.

## Scope and limits

- Groups, users, applications, proxy and OAuth2 providers are fully managed;
  outposts and keypairs are observed and can be adopted, and a keypair can be
  created. Roles, parents and policy bindings are not managed here.
- A user is a **seed**: created when missing, never updated and never deleted.
  The profile, the credentials and the account belong to the person, so a
  reconcile must not turn a UI edit into drift.
- An application is fully managed: created, updated and deleted. Its ownership
  comes from the core's record, never from a marker (ADR-037).
- Providers and outposts have no marker field either; the core's record owns
  them. A proxy provider is only reachable while the embedded outpost lists it,
  and that list is declared as an outpost attribute or in a blueprint.
- An OAuth2 provider's secret is write-only: nod can create one from
  `client_secret_env`, and can adopt an existing one without reading or
  changing the secret it already has. It never reports it.
- The API is `/api/v3/core/` for groups, users and applications,
  `/api/v3/providers/all/` for the provider type map and name resolution,
  `/api/v3/providers/proxy/` for proxy providers,
  `/api/v3/providers/oauth2/` for OAuth2 providers,
  `/api/v3/propertymappings/provider/scope/` for scope mappings,
  `/api/v3/outposts/instances/` for outposts and `/api/v3/flows/instances/` for
  flow slugs; pagination follows `pagination.next` to the end. The provider was
  written against Authentik 2026.5.6 and speaks API v3.
- A declared username or provider name that does not exist fails the change
  instead of being skipped: silently dropping a member is worse than a loud
  error.

## Conformance

```bash
nod provider test ./nod-provider-authentik --settings settings.json --env AUTHENTIK_TOKEN
```

The provider's own tests cover the model, planning, subset semantics, pagination,
the OAuth2 type filter, the absence of the client secret in observed resources
and the error classes (401/403, 429, 5xx). The protocol test drives the real
binary against a fake API and proves the handshake, an observation that
contains an OAuth2 provider without its secret, planning and an empty apply.
