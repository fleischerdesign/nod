# Writing a provider

A provider is an executable that speaks JSON Lines over stdin/stdout. It does
not import nod, and it can be written in any language.

This directory contains a complete provider in ~130 lines of Python with no
dependencies. Run it:

```bash
echo '{"protocol":1}' | ./nod-provider-example.py
```

## The four operations

**Handshake** — the core announces its protocol version; the provider answers
with who it is and what it can do.

```jsonc
→ {"protocol": 1}
← {"provider": "example", "version": "0.1.0", "protocol": 1,
   "capabilities": ["example.record"], "ownership": "marker",
   "settings_schema": {"type": "object", "required": ["state"], "...": "..."}}
```

**Observe** — report what exists, with an `owned` flag per resource.

```jsonc
→ {"op": "observe", "config": {"state": "/tmp/state.json"}}
← {"resources": [{"kind": "example.record", "id": "one",
                  "attributes": {"value": "1"}, "owned": true}],
   "conditions": []}
```

**Plan** — compare the declaration with the observation and propose changes.
`owned` and `destructive` are mandatory: the core refuses what it cannot
verify.

```jsonc
→ {"op": "plan", "desired": [...], "observed": [...]}
← {"changes": [{"kind": "create", "owned": true, "destructive": false,
                "resource": {"kind": "example.record", "id": "one", "attributes": {}}}]}
```

**Apply** — receives only the changes the core approved.

```jsonc
→ {"op": "apply", "changes": [...]}
← {"applied": [{"id": "one", "ok": true}], "conditions": []}
```

## Rules the core relies on

- **One JSON object per line.** stdout is protocol; stderr is log. A non-JSON
  line on stdout is a protocol violation, not a warning.
- **Never answer an error with an empty result.** Use
  `{"error": {"kind": "auth", "message": "…"}}`. An empty `resources` list
  would read as an empty world.
- **`owned` is not optional.** A resource without the flag is foreign, and the
  core will never delete it.
- **Idempotency.** Applying the same plan twice must not create duplicates.
- **Unknown fields are ignored.** Both sides may add optional fields; a required
  field is a protocol major change.

## Running it from a configuration

Declare the provider once and `nod reconcile` resolves it by name:

```toml
# .nod.toml
[providers.example]
command = "/path/to/nod-provider-example"

[providers.example.settings]
state = "/tmp/example-provider.json"
```

```bash
nod reconcile example --desired plan.json
```

`--command` and `--settings` override the configured values for one run. Nothing
is discovered: a provider exists because the configuration names it.

## Testing

```bash
nod provider test ./nod-provider-example.py
```

The conformance command checks the handshake, a valid observe, a plan that obeys
ownership, and an apply that accepts an empty change set. Anything beyond that
belongs to the provider's own tests against its API.

Set `NOD_PROVIDER_TRACE=1` to see every line in both directions.
