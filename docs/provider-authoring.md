# Writing a provider

A provider is a **process**, not a library. It speaks JSON Lines over
stdin/stdout; `nod` owns the protocol and the policy, the provider owns the API.
Any language with a JSON parser can implement one.

This document is the contract a provider must keep. It is deliberately smaller
than the API it wraps.

## The conversation

```
core → {"protocol": 1}
prov ← {"provider": "example", "version": "0.1.0", "protocol": 1, ...}

core → {"op": "observe", "config": {...}}
prov ← {"resources": [...], "conditions": [...]}

core → {"op": "plan", "config": {...}, "desired": [...], "observed": [...]}
prov ← {"changes": [...], "conditions": [...]}

core → {"op": "apply", "config": {...}, "changes": [...]}
prov ← {"applied": [...], "conditions": [...]}

core → {"op": "adopt", "config": {...}, "resources": [...]}   # optional
prov ← {"adopted": [...], "conditions": [...]}
```

Rules:

- One JSON object per line. stdout is protocol, stderr is log. A banner on
  stdout is a protocol violation, not a warning.
- The core starts the process, sends one request, reads one response, and
  repeats. It never parses a provider's stderr.
- Unknown operations are a bug; the core only sends what the handshake
  declares.

## The handshake

The first exchange, before anything is observed:

```json
{
  "provider": "cloudflare",
  "version": "0.1.0",
  "protocol": 1,
  "capabilities": ["dns.record", "zone.settings"],
  "operations": ["adopt"],
  "ownership": "marker",
  "settings_schema": {
    "type": "object",
    "required": ["zone"],
    "properties": {
      "zone": {"type": "string"},
      "token_env": {"type": "string", "default": "CLOUDFLARE_API_TOKEN"}
    }
  }
}
```

- `protocol` is the major version. A provider that speaks another major is
  rejected before it starts.
- `capabilities` lists the resource kinds the provider manages. When the list
  is non-empty, any observed resource or proposed change of another kind is a
  protocol violation.
- `operations` lists optional operations. Today only `adopt` exists. An old
  core ignores the field, an old provider omits it; this is additive.
- `ownership` is `marker`, `namespace` or `none`.
- `settings_schema` describes `config`. The core checks its **shape** and that
  the provided settings contain the required keys; the provider remains the
  authority on types and semantics.

## The manifest

`provider.json` sits next to the executable. `nod provider list` reads it
without running the provider; `nod provider test` compares it with the
handshake.

```json
{
  "name": "cloudflare",
  "version": "0.1.0",
  "protocol": 1,
  "capabilities": ["dns.record", "zone.settings"],
  "operations": ["adopt"],
  "ownership": "marker",
  "description": "Cloudflare DNS records and edge settings",
  "sandbox": {
    "desired": [
      {
        "kind": "dns.record",
        "id": "TXT:_nod-conformance",
        "attributes": {"type": "TXT", "name": "_nod-conformance", "content": "ok"}
      }
    ]
  }
}
```

A manifest that names an operation the handshake does not offer is an error.
`nod provider list` must work without a token or a network.

## Resources

```json
{
  "kind": "dns.record",
  "id": "A:www.example.com",
  "attributes": {"type": "A", "name": "www.example.com", "content": "192.0.2.1", "ttl": 1, "proxied": false},
  "owned": true
}
```

- `id` is stable within the provider instance. For DNS records, `TYPE:name`.
- `attributes` is provider-specific. Only the keys a declaration controls are
  compared; bookkeeping fields (record ids, markers) are not desired state.
- `owned` is the provider's answer to "did this fleet create this?". When in
  doubt, `false`: foreign is the safe default.

## Ownership

| Mode | Meaning | Adoption |
|---|---|---|
| `marker` | every managed resource carries a marker: a comment, a label, a tag | possible |
| `namespace` | the whole namespace belongs to the fleet; an explicit opt-in | not needed |
| `none` | read-only; the provider may not propose a change | impossible |

With `marker`, a resource without the marker is foreign. `plan` may propose an
`update` with `owned=false` for a foreign resource so the core reports the
conflict; it must never propose a `delete` for one.

Adoption is an optional operation. It stamps the marker on resources the core
deemed eligible (declared and matching); it changes nothing else. The provider
does not interpret comments, labels or names from other tools.

## Operations

### `observe`

Returns the **complete** world or an error. Pagination is the provider's
business; a provider that cannot enumerate everything reports an error rather
than a truncated list. A partial world read as complete is how a reconciler
plans deletions for resources it never saw.

### `plan`

Pure: no writes, no side effects. Compare `desired` with `observed` and emit:

```json
{"kind": "create", "owned": true, "destructive": false, "resource": {...}}
{"kind": "update", "owned": true, "destructive": false, "resource": {...}}
{"kind": "delete", "owned": true, "destructive": true, "resource": {...}}
```

`owned` and `destructive` are mandatory. A `delete` is always destructive. A
plan that would delete a foreign resource is wrong at the source.

### `apply`

Receives only the changes the core approved. Answer per change:

```json
{"applied": [
  {"id": "A:www.example.com", "ok": true},
  {"id": "TXT:old.example.com", "ok": false,
   "error": {"kind": "auth", "message": "token expired"}}
], "conditions": []}
```

- `ok` is a statement, not a hope. A change that did not happen is `ok:false`.
- A missing result for an approved change is treated as a failure.
- Partial success is allowed; the core records the undo only for `ok` changes.

### `adopt`

Optional. Stamps the provider's own marker on the resources the core lists.
Writes only the marker — not content, TTL, proxy state or tags. Per-resource
results use the same `{id, ok, message?, error?}` shape.

## Errors

An operation may fail, and a failure is its own answer:

```json
{"error": {"kind": "rate-limited", "message": "slow down", "retry_after_secs": 30}}
```

| Kind | Meaning | Core reaction |
|---|---|---|
| `auth` | credentials rejected | operator action, no retry storm |
| `rate-limited` | slow down; optional `retry_after_secs` | backoff |
| `config` | settings are wrong | fail the run |
| `transient` | 5xx, transport, timeout | retry with backoff |
| `protocol` | the provider or the core broke the contract | fail loudly |

Never answer an error with an empty result: `resources: []` reads as an empty
world, and an unobserved world is not a healthy one.

## Settings and secrets

`config` is opaque to the core. Validate it against `settings_schema`; return a
`config` error for a missing or invalid key.

Secrets travel by **name**, never by value in argv or logs:

```json
{"zone": "example.com", "token_env": "CLOUDFLARE_API_TOKEN"}
```

The core forwards the declared environment variables from its own environment.
A provider never writes a token to stdout, stderr, a plan or a resource.

## Limits

- One operation is bounded by the declared `timeout_secs` (default 60); the
  handshake has its own 5-second bound.
- One response frame may not exceed 16 MiB. A larger frame is rejected.
- The core clears the environment except an explicit allowlist; do not depend
  on ambient variables.

## Singletons

A kind may be a singleton, such as a zone's edge settings:

```json
{"kind": "zone.settings", "id": "zone.settings", "attributes": {
  "ssl": "strict", "always_use_https": "on", "min_tls_version": "1.3"
}, "owned": true}
```

Rules: one id, always observed, owned by construction (the declaration is the
only writer), only `update` is ever proposed, and absence from the declaration
means "do not change", never "reset to defaults". See ADR-033.

## Conformance

```
nod provider test /path/to/provider [--settings settings.json] [--env NAME]
nod provider test /path/to/provider --write
```

The suite proves: the handshake, the manifest agreement, a valid observe, a
plan that obeys ownership, a declared `adopt` that answers an empty request,
that no declared secret env value appears in any response, and — with `--write`
— create, idempotency and cleanup in a **sandbox**. After apply the fixture
must be visible in the next observe: a provider that reports `ok` without
writing anything fails there, not only at the next plan.

`--write` requires a `sandbox` fixture in the manifest: a namespace the test is
allowed to destroy. Cleanup deletes exactly the fixture ids, never the whole
namespace. Do not point it at a production zone; a provider that cannot offer a
disposable namespace should omit `sandbox` and leave `--write` unavailable.

The suite does not prove the API integration. That needs the provider's own
fake API or a real sandbox account; the error taxonomy (401/403 → `auth`,
429 → `rate-limited`, 5xx → `transient`) is part of that provider-owned proof.

## Packaging

A provider is a separate package, not a module of the core. The core must not
gain an HTTP client because one provider needs one.

- Nix users get a store path from the module; non-Nix users pass an absolute
  path. There is no PATH discovery and no auto-install.
- `provider.json` lives next to the binary.
- Pin the provider's dependencies; a provider that fetches its own code at
  runtime is a supply-chain hazard.

## Checklist

- [ ] Handshake matches `provider.json`; both declare the same protocol.
- [ ] `observe` returns the complete world or an error.
- [ ] `plan` is pure and never proposes a delete for a foreign resource.
- [ ] Every change carries `owned` and `destructive`.
- [ ] `apply` reports `ok:false` for a change that did not happen.
- [ ] The same declaration produces an empty plan after apply (idempotent).
- [ ] 401/403 is `auth`, 429 is `rate-limited`, 5xx is `transient`.
- [ ] `adopt` writes only the marker.
- [ ] A singleton is update-only and absent means unchanged.
- [ ] The sandbox fixture is visible after apply.
- [ ] No secret in stdout, stderr, a plan or a resource; the suite scans
  responses for every declared `--env` value.
- [ ] `provider test --write` passes in a disposable sandbox.

## Reference implementations

- `providers/example` — a minimal Python provider with a JSON file as its world.
- `providers/cloudflare` — a real API adapter: marker ownership, `adopt`, and a
  `zone.settings` singleton.
