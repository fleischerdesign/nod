# ADR-030: Provider Loading and Protocol

- **Status:** Proposed
- **Date:** 2026-10-22
- **Deciders:** maintainers
- **Relates to:** ADR-004 (configuration hierarchy), ADR-027 (exit status), ADR-029 (health verdicts)

## Context

`nod` reconciles declared Nix state onto hosts. The same declaration also
describes resources that never reach a host: DNS records, SSO providers, proxy
routes, buckets. Those live behind provider APIs, and no Nix-native tool
reconciles them without a second state file.

The design decision is not *whether* to reconcile them, but *how providers are
loaded and spoken to*. Two constraints shape it:

1. **Third parties must be able to write providers.** A provider for an internal
   CMDB or a regional DNS host will never be first-party code, and it must not
   require rebuilding `nod`.
2. **The core's guarantees must survive a hostile or buggy provider.** "Never
   delete a resource we do not own" cannot be a provider's opinion.

The obvious implementation - a Rust trait plus `dlopen`, or a plugin ABI - fails
both. It locks providers to one language and one compiler version, and it puts
provider crashes and undefined behaviour inside the reconciler process.

## Decision

### 1. The process boundary is the ABI

A provider is an executable. The core starts it, speaks JSON Lines over
stdin/stdout, and kills it when the conversation ends or a timeout fires.

- No shared library, no `dlopen`, no stable Rust ABI.
- Any language, any runtime, any build system.
- A provider crash is a failed provider, not a failed reconciler.

stdout carries protocol messages and nothing else; stderr carries logs. A line
that is not valid JSON on stdout is a protocol violation, not a warning: silent
tolerance is how a provider that logs to the wrong stream corrupts a plan.

### 2. Resolution is declared, never discovered

A provider exists for `nod` because the configuration names it. There is no PATH
scan, no auto-install, no implicit directory of plugins.

```toml
[providers.cloudflare]
command = "/nix/store/…-nod-provider-cloudflare/bin/nod-provider-cloudflare"
protocol = 1
settings = { zone = "example.com" }
```

Three sources, in order of preference:

| Source | Audience | Form |
|---|---|---|
| Nix package | Nix users | the module renders the store path |
| Local path | development | `command = "./target/debug/nod-provider-x"` |
| Standalone binary | non-Nix users | absolute path, optional `sha256` |

The Nix module and a hand-written file render the **same** structure; the only
difference is who writes it (ADR-004).

### 3. Handshake before anything is observed

```jsonc
→ {"protocol": 1}
← {"provider": "cloudflare", "version": "0.3.1", "protocol": 1,
   "capabilities": ["dns.record"], "ownership": "marker",
   "settings_schema": { … }}
```

The handshake must answer within 5 s, or the process is killed and the provider
is reported as broken. The core validates the user's `settings` against
`settings_schema` before the first observation; a typo is a configuration error
with a path, never a silently applied default.

A provider whose `protocol` major differs from the core's is rejected **before**
it is started.

### 4. The operations

```
observe(config)                    → resources[] + conditions[]
plan(desired, observed)            → changes[]
apply(changes the core approved)   → applied[]
```

Rules that make the conversation safe:

- Every change carries `owned` and `destructive`. A response without both is
  invalid.
- An operation may fail, and a failure is its own answer:
  `{"error": {"kind": "auth", "message": "…"}}`. It is never an empty result.
  `resources: []` would read as an empty world, and an unobserved world is not
  a healthy one — the same distinction ADR-029 draws for hosts.
- `apply` receives **only** the changes the core approved. A provider cannot
  smuggle a deletion the core did not see.
- `verify` is not an operation. The core calls `observe` again and compares;
  "the API returned 200" is not verification.
- Pagination is part of the protocol from the first version (`has_more`,
  `cursor`). A zone with 100k records must not require a breaking change later.

### 5. The core keeps the policy

The provider describes the world; the core decides what happens to it.

| Decision | Owner |
|---|---|
| What resources exist | provider |
| What should change | provider (as a proposal) |
| Whether an unowned resource may be deleted | **core** |
| Whether a destructive change runs at all | **core** |
| Dry-run (never calls `apply`) | **core** |
| Ownership mode (`marker`, `namespace`, `none`) | declared in config, enforced by core |

`settings` are opaque to the core: validated against the provider's schema and
passed through. The core interprets none of them. The invariants are not
settings.

### 6. Ownership

- `marker`: every resource carries a marker (TXT record, label, description).
- `namespace`: the whole namespace belongs to the fleet - an explicit opt-in.
- `none`: read-only provider; it may never propose a change.

Without a marker, a resource is foreign. Foreign resources are reported, never
deleted, never modified.

### 7. Isolation

- The provider inherits no environment beyond an explicit allowlist.
- Secrets arrive through a file or a file descriptor, never through argv:
  argv is visible in `ps`.
- Working directory is a temporary directory, not the flake.
- Timeout per operation; then `SIGKILL`. A hanging provider costs one cycle, not
  the reconciler.
- Per-provider concurrency defaults to 1; a token bucket and `Retry-After`
  handling live in the core, because rate limits are a property of the
  conversation, not of one provider implementation.

### 8. Versioning

Three independent numbers:

| Number | Meaning | Enforced by |
|---|---|---|
| Protocol major | message shape | core rejects mismatches before start |
| Provider version | the implementation | reported by `nod provider list` |
| Settings schema version | the user-facing config | validated per run |

Protocol minor changes are additive; a major change is a new contract.

### 9. The CLI

```
nod provider list           resolved providers, source, version, protocol, status
nod provider test NAME      conformance suite against a fake API
nod provider schema NAME    print the settings schema
```

`list` answers the question plugins usually cannot: what is installed, where did
it come from, and is it compatible.

### 10. Conformance is a test, not a promise

`nod provider test` asserts the properties the core depends on: protocol
validity, ownership, error shape, and the handshake/manifest agreement. It does
not write unless asked.

`--write` runs a full cycle - create, verify, observe again, delete - and proves
idempotency: the same declaration must produce an empty plan after it was
applied. It requires a `sandbox` fixture in the provider's manifest, because a
write test needs a namespace that may be destroyed, and only the provider knows
which resources express that. Cleanup deletes exactly the fixture ids, never the
whole namespace: a conformance run must not be able to empty a real zone.

A provider that fails the suite is not compatible, however well it works
against the real API.

## Consequences

- Providers are language-agnostic and independently versioned; the cost is a
  process per run and a JSON protocol to maintain.
- The protocol is public API from the first release. This cannot be introduced
  later without a rewrite, which is why it is an ADR now.
- The core grows a supervisor (spawn, timeout, kill, parse) that must be tested
  as carefully as the deployers are today.
- Third-party providers can be malicious within their credentials; capabilities
  and least privilege limit the blast radius, not eliminate it.

## Rejected alternatives

- **Rust trait plus `dlopen`.** Locks providers to Rust and a compiler version;
  crashes the reconciler.
- **PATH discovery or auto-install.** Non-deterministic, and a reconciler that
  fetches its own code at runtime is a supply-chain hazard.
- **A plugin registry before a third-party provider exists.** A registry for an
  ecosystem nobody has joined is overhead with a URL.
- **Letting the provider decide about deletions.** The one guarantee the product
  cannot delegate.

## Implementation

1. Protocol types, supervisor, fake provider (a shell script) and the
   conformance suite.
2. `nod provider list/test/schema` over the supervisor.
3. Cloudflare adapter as the first real provider, reusing the existing
   `cloudflare-sync` logic.
4. `nod reconcile --dry-run` as the first user-visible command; `--apply`
   afterwards.
5. A third-party provider, written by someone who is not a maintainer, as the
   proof that the seam holds.

## Open questions

- Should `nod provider add` write configuration, or stay read-only?
- Do capabilities belong in the handshake only, or also in the package metadata
  so a Nix module can type-check them?
- Is `namespace` ownership safe enough to ship, or does it need a second
  confirmation outside the config file?
