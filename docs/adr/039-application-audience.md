# ADR-039: Applications declare their audience

- **Status:** Accepted
- **Date:** 2026-10-24
- **Deciders:** maintainers
- **Refines:** ADR-035 (ownership per kind), ADR-037 (ownership by evidence),
  ADR-038 (provider authority).

## Context

Authentik decides access to an application through `PolicyBinding` rows on the
application's `PolicyBindingModel`. Without any binding, `core_default_app_access`
(default `True`) makes the application available to every authenticated user. The
fleet's blueprints therefore create a group binding per declared audience; an
application a provider creates without a binding would be **open**.

An application is a resource with attributes. Its audience is part of what the
declaration asks for, not a separate object a human maintains: the same diff
that renames the application should also decide who may launch it.

## Decision

### 1. `access_groups` is an application attribute

The attribute lists group names. The provider observes the application's enabled
group bindings, maps group primary keys to names, and compares like any other
declared key: absent means unmanaged, a different list means `update`.

### 2. Only group bindings are managed

User and policy bindings are a different decision — an expression policy, a
single person's entitlement — and stay untouched. A disabled binding grants
nothing, so it does not appear in `access_groups`; when its group is declared, an
enabled binding is created first and the disabled one is then removed.

### 3. The declared list is complete for groups

A group binding outside the declaration is removed. That is the point of the
attribute: the audience is not a merge of history and intent. The plan shows an
`update`, not individual binding changes; the declared list is reviewable in the
diff that carries it.

The deletion budget does not apply here: the audience is declared state, not a
fact derived at runtime. A typo is caught by review, like any other attribute.

### 4. Grants before removals; a half-created application is removed

A reconcile creates missing bindings before deleting extra ones, so a failure
never leaves the application less accessible than either state. If creating an
application succeeds but its bindings fail, the provider deletes the application
again: an unowned, open application is the one state nobody can repair from the
declaration. If that compensating delete also fails, the change reports it
instead of pretending.

### 5. An empty list is a declaration

`access_groups: []` removes every group binding and deliberately returns the
application to `core_default_app_access`. That is a legitimate choice for a
public application, and the provider does not second-guess it.

## Consequences

- An application and its audience are one resource and one diff.
- Access changes appear as an application update; the global deletion cap does
  not cover them, by design.
- A foreign application is never touched: its bindings are only reconciled when
  the application is owned (ADR-037).
- The provider needs binding read and write access; the account authority stays
  one grant (ADR-038).

## Alternatives

| Alternative | Why not |
|---|---|
| a separate `authentik.binding` kind | the application could exist open between two changes; ordering and ownership double |
| managing user and policy bindings too | they encode expressions and single-user entitlements; a different feature |
| leaving access to the blueprints | two writers for one object, and the audience drifts from the application |
| refusing an empty list | a public application is a valid declaration |
