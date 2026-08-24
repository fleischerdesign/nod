# ADR-007: Effective Connection Profile Flows Through the Deploy Port

- **Status:** Accepted
- **Date:** nod v2 hardening
- **Deciders:** nod maintainers
- **Supersedes:** n/a (resolves a defect in ADR-001's port boundary, see below)

## Context

Audit (ADR-001) surfaced a silent configuration-loss defect in the adapter seam.
`ConfigStorePort::resolve(host)` returns a fully-resolved `SshProfile` carrying every
connection knob (`identity_file`, `proxy_jump`, `proxy_command`, `sudo`,
`timeout_secs`, `connect_timeout_secs`, `extra_ssh_args`, `allow_insecure`, plus
`user`/`port`). That method is well tested.

However, the transport adapters (`SshCliDeployer`, and the SSH path of `DeployerPort`)
re-derive the profile themselves via `SshProfile::for_host(host)`, which only
materializes `user`, `port`, and `sudo = host.is_local`. Every other resolved setting is
silently discarded the moment a deploy starts. A host configured with `identity_file`,
`proxy_jump`, or a non-default port is connected to as `root@host:22` with no identity
file. The contract documented on `apply_to` — "so downstream adapters derive the
effective profile from it" — is not honoured anywhere on the connection path.

Root cause is architectural: **configuration resolution lives in the wrong place.**
Adapters are responsible for transport (argv construction, subprocess lifecycle), not
for deciding *which* connection settings apply. The "which profile" decision is
application/domain knowledge, and the profile is an already-resolved value object that
should be passed in.

## Decision

**The resolved `SshProfile` becomes an explicit argument to the deploy port**, flowing
from the caller (use case / command), which obtains it from `ConfigStorePort::resolve`.

- `DeployerPort` transport methods accept `&SshProfile` alongside `&HostEntity`.
- Adapters stop calling `SshProfile::for_host` internally; they consume only the passed
  profile (this is the DIP: adapters are dumb transports).
- `HostEntity::ssh_profile()` (domain convenience that builds a *primitive* profile) is
  removed or restricted to the non-resolved/local case, so it cannot be mistaken for the
  effective profile. Where a use case has no config store binding, it falls back to a
  documented local-only profile rather than silently widening.

This keeps the port boundary intact (domain still defines the port; adapters implement
it) while moving the *knowledge* of effective settings into the layer that owns
resolution.

## Consequences

- Adapters become purely transport: their test surface shrinks to argv construction.
- The caller owns resolution, so every command path that reaches a deploy must resolve
  the profile first. A regression test asserts
  `resolve(h)` equals the profile the transport actually receives.
- `SshProfile` remains immutable; no field is added — the change is about *flow*, not
  shape. This deliberately reuses the existing, tested value object (DRY: no new schema).

## Alternatives

- **Inject `ConfigStorePort` into the adapters** so they resolve internally. Rejected:
  binds transport to config (SRP), makes adapters do work the domain should own, and
  complicates adapter construction/tests.
- **Enrich `HostEntity` with the full profile** (make `apply_to` materialize it). This was
  the audit's option (a). Rejected as a primary mechanism: `HostEntity` is a discovery
  entity, not a transport descriptor; embedding resolved secrets/credentials into it
  widens its serialization surface and couples discovery to connection policy. The
  explicit-argument form is more honest about ownership. `apply_to` may remain for the
  non-resolved discovery case but is not the authoritative connection path.
