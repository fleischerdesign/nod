# nod-provider-authentik

Authentik groups for [nod](https://github.com/fleischerdesign/nod), spoken over
the provider protocol (JSON Lines on stdin/stdout).

The provider owns `authentik.group` objects: it observes them, plans the
difference against a declaration, applies it, and adopts existing groups on
request. Users, roles, parents and bindings stay outside its scope.

## Capabilities

| Kind | Id | Attributes |
|---|---|---|
| `authentik.group` | group name (unique in Authentik) | `is_superuser`, `description`, `users` |
| `authentik.user` | username | `name`, `email` |

`users` lists login names. `description` and `users` are written into the group's
free-form `attributes` and membership respectively; `is_superuser` is the group's
own flag.

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
      "attributes": {
        "is_superuser": false,
        "description": "Media consumers",
        "users": ["philipp"]
      }
    }
  ]
}
```

A missing group is planned as `create`; an owned group whose declared attributes
differ is `update`; an owned group outside the declaration is `delete`. Foreign
groups are never updated or deleted: adoption is an explicit operation, not a
reconcile side effect.

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
| `observe` | Reads every group, paginated, with its members. |
| `plan` | Computes `create`/`update`/`delete` against the declaration. |
| `apply` | Writes the plan. Refuses every change to an unowned resource. |
| `adopt` | Stamps the marker on an existing group; nothing else changes. |

Deletion is the core's decision, not the provider's: the core's `policy.protect`
and `policy.max_deletions` bound what a plan may remove, and a plan over the cap
is refused as a whole.

## Permissions

The manifest and the handshake name what the token must be allowed to do:

| Permission | Needed for |
|---|---|
| `authentik_core.view_group` | `observe`, `plan`, `apply` |
| `authentik_core.view_user` | resolving declared members to primary keys |
| `authentik_core.change_group` | `update`, `adopt` |
| `authentik_core.add_group` | `create` |
| `authentik_core.add_user` | `create` of a user |
| `authentik_core.change_user` | `adopt` of a user (the marker) |
| `authentik_core.delete_group` | `delete`, only when the core's policy allows it |

`observe` checks the two read permissions by using them: a token without
`view_group` fails the observation with the permission named, and a token
without `view_user` still observes and reports a condition, so
`nod provider status` shows the gap before the first write. The write
permissions cannot be probed without writing; a rejected write names the
permission it needed instead of saying "credentials rejected".

## Scope and limits

- Groups only. Users, roles, parents, policy bindings and applications are not
  managed here.
- A user is a **seed**: created when missing, never updated and never deleted.
  The profile, the credentials and the account belong to the person, so a
  reconcile must not turn a UI edit into drift.
- The API is `/api/v3/core/groups/`; pagination follows `pagination.next` to the
  end. The provider was written against Authentik 2026.5.6 and speaks API v3.
- A declared username that does not exist fails the change instead of being
  skipped: silently dropping a member is worse than a loud error.

## Conformance

```bash
nod provider test ./nod-provider-authentik --settings settings.json --env AUTHENTIK_TOKEN
```

The provider's own tests cover the model, planning, subset semantics, pagination
and the error classes (401/403, 429, 5xx).
