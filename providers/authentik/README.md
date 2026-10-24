# nod-provider-authentik

Authentik groups, users and applications for [nod](https://github.com/fleischerdesign/nod),
spoken over the provider protocol (JSON Lines on stdin/stdout).

The provider owns `authentik.group`, `authentik.user` and `authentik.application`
objects: it observes them, plans the difference against a declaration, applies
it, and adopts existing groups and users on request. Roles, parents and policy
bindings stay outside its scope.

## Capabilities

| Kind | Id | Attributes |
|---|---|---|
| `authentik.group` | group name (unique in Authentik) | `is_superuser`, `description`, `users` |
| `authentik.user` | username | `name`, `email` |
| `authentik.application` | slug (unique in Authentik) | `name`, `provider`, `meta_launch_url`, `meta_description`, `meta_publisher`, `open_in_new_tab`, `policy_engine_mode`, `group`, `meta_hide` |

`users` lists login names. `description` and `users` are written into the group's
free-form `attributes` and membership respectively; `is_superuser` is the group's
own flag.

`provider` names a provider the way a human knows it (for example
`grafana-oauth`); the provider resolves it to the primary key the API expects
and fails the change when the name is unknown. An empty string detaches it.
`meta_icon` and the backchannel providers are not managed: the first is a file
upload, the second a relation this provider does not own.

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
| `observe` | Reads every group, user and application, paginated. |
| `plan` | Computes `create`/`update`/`delete` against the declaration. |
| `apply` | Writes the plan. Refuses every change to an unowned resource. |
| `adopt` | Stamps the marker on an existing group or user; nothing else changes. Applications are refused. |

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

- Groups, users and applications. Roles, parents, policy bindings, providers
  and outposts are not managed here.
- A user is a **seed**: created when missing, never updated and never deleted.
  The profile, the credentials and the account belong to the person, so a
  reconcile must not turn a UI edit into drift.
- An application is fully managed: created, updated and deleted. Its ownership
  comes from the core's record, never from a marker (ADR-037).
- The API is `/api/v3/core/` for groups, users and applications and
  `/api/v3/providers/all/` for the provider-name resolution; pagination follows
  `pagination.next` to the end. The provider was written against Authentik
  2026.5.6 and speaks API v3.
- A declared username or provider name that does not exist fails the change
  instead of being skipped: silently dropping a member is worse than a loud
  error.

## Conformance

```bash
nod provider test ./nod-provider-authentik --settings settings.json --env AUTHENTIK_TOKEN
```

The provider's own tests cover the model, planning, subset semantics, pagination
and the error classes (401/403, 429, 5xx).
