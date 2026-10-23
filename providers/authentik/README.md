# nod-provider-authentik

Authentik groups for [nod](https://github.com/fleischerdesign/nod), spoken over
the provider protocol (JSON Lines on stdin/stdout).

**Stage 0: read-only.** `observe` and `plan` are implemented; `apply` refuses a
non-empty change set. The core's `apply = false` is the second guard. Adoption
and writes land in later stages, behind their own conformance runs.

## Capabilities

| Kind | Id | Attributes |
|---|---|---|
| `authentik.group` | group name (unique in Authentik) | `is_superuser` |

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

## Desired state

```json
{
  "resources": [
    {
      "kind": "authentik.group",
      "id": "media",
      "attributes": { "is_superuser": false }
    }
  ]
}
```

A missing group is planned as `create`; an owned group whose attributes differ is
`update`; an owned group outside the declaration is `delete`. Foreign groups are
never updated or deleted: adoption is an explicit operation, not a reconcile
side effect.

## Token

A read-only run needs an Authentik API token with `view_group` (and nothing
else). A write stage will need `add_group`, `change_group` and `delete_group`;
grant them only when `apply` is enabled.

## Scope and limits

- Groups only. Users, roles, parents and memberships are not part of this stage.
- `apply` is not implemented; the binary refuses changes loudly.
- `adopt` is not offered yet; the marker is read, not written.
- The API is `/api/v3/core/groups/`; pagination follows `pagination.next` to the
  end. The provider was written against Authentik 2026.5.6 and speaks API v3.

## Conformance

```bash
nod provider test ./nod-provider-authentik --settings settings.json --env AUTHENTIK_TOKEN
```

`--write` is unavailable: the manifest has no `sandbox` and the build does not
write. The provider's own tests cover the model, planning, pagination and the
error classes (401/403, 429, 5xx).
