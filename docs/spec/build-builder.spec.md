# nod — Build via Builder Host (`nod build --builder`)

> Gherkin feature specification for distributed remote compilation through a
> configured fleet host (ADR-005 one-host-at-a-time shape, ADR-006 unified
> target selector, ADR-001 port/sei). The builder is a **configured NixOS host
> of the fleet**, selected exactly like any other target; the Nix `--builders`
> SSH transport remains encapsulated in the infrastructure evaluator adapter.
> Each `Feature` is behavior-observable against **mock adapters** — no real
> Nix/SSH/network is required.

---

## Feature: `nod build --builder <HOST>` — remote build through a fleet host

```gherkin
@build @builder @distributed
Feature: A built closure can be compiled on a configured builder host

  Scenario: a builder host is selected through the unified selector
    Given a fleet with hosts `web-01` and `buildy` declared in the flake
      And `--builders` is available on the local `nix` and the builder is a Nix
      host reachable via SSH
    When `nod build web-01 --builder buildy` runs
    Then exactly one builder host `buildy` is resolved via the unified target
      selector (ADR-006)
    And the `nix build` invocation for `web-01` passes `--builders ssh://buildy`
    And the built closure is returned to the requesting store (Nix transports
      the result back automatically)
    And the host is reported as `Prepared` without transfer or activation
    And no SSH flag, user, port or identity plumbing leaks into the CLI or
      domain layers (the `ssh://` URI form is confined to the infra adapter)

  Scenario: a zero-match builder target is a typed error
    Given a fleet with no host named `nowhere`
    When `nod build web-01 --builder nowhere` runs
    Then a typed `no hosts matched target 'nowhere'` config error is raised
    And nothing is built

  Scenario: a multi-match builder selector is a typed error
    Given a fleet with hosts `web-01`, `web-02` (glob `web-*`)
    When `nod build web-01 --builder web-*` runs
    Then a typed `multiple hosts matched` config error is raised listing the
      candidates
    And nothing is built

  Scenario: without `--builder` the build stays local
    Given a fleet with host `web-01` with no `config.nod.build.buildHost`
    When `nod build web-01` runs (no `--builder`)
    Then the `nix build` invocation has no `--builders` argument
    And the build is a plain local build (today's behavior)

  Scenario: the flake `buildHost` default supplies the builder
    Given host `web-01` declares `config.nod.build.buildHost = "buildy"`
    When `nod build web-01` runs (no `--builder`)
    Then the builder host defaults to `buildy` (lower cascade tier)
    And the `nix build` invocation for `web-01` passes `--builders ssh://buildy`

  Scenario: an explicit `--builder` overrides the flake default
    Given host `web-01` declares `config.nod.build.buildHost = "buildy"`
      And the fleet additionally declares `fast-builder`
    When `nod build web-01 --builder fast-builder` runs
    Then the explicit `--builder fast-builder` wins (highest cascade tier)
    And the flake `buildHost` default is ignored

  Scenario: a builder resolving to the local host is a plain local build
    Given the local machine is a configured fleet host
    When `nod build web-01 --builder local` runs
    Then `--builders` is not appended (identity to today's local behavior)

  Scenario: `--out-link` stays orthogonal to the builder
    Given `nod build web-01 --builder buildy --out-link /tmp/result` runs
    Then the closure is built via `--builders ssh://buildy`
    And the out-link symlink is created at `/tmp/result` on the requesting
      store exactly as in a plain local build
```

---

## Cascade tier (builder selection)

```
Operator override --builder <HOST>   ← highest (CLI, ADR-004 tier 1)
Flake config.nod.build.buildHost     ← host default (ADR-004 tier 3)
(no builder source)                  → plain local build (today's behavior)
```

Consistent with the existing four-tier override cascade (`--user`/`--port`
beat `.nod.toml` beat flake beat compiled defaults). `--builder` is the
per-run ad-hoc override; `buildHost` is the stationary per-host default; the
two are one decision at different granularity, not two parallel concepts.

---

## Edge cases

Covered:
- Zero-match builder selector → typed `no hosts matched` [scenario 2]
- Multi-match selector (`web-*`) → typed `multiple hosts matched` [scenario 3]
- No builder source → plain local build [scenario 4]
- Flake `buildHost` default → applied [scenario 5]
- CLI `--builder` overrides flake default [scenario 6]
- Builder resolves local → identity to local build [scenario 7]
- `--out-link` orthogonal to builder [scenario 8]

Not covered (explicitly out of scope for this change):
- **Multi-host builds with per-host different `buildHost` defaults** — the
  cascade lower tier applies when the build run resolves to **exactly one**
  built target; a fleet-wide glob build with mixed per-host builders is not
  supported in this iteration (would need per-host builder plumbing through
  the use case; the roadmap's `single-host build` shape does not require it).
- **Builder hosts that are not configured NixOS fleet hosts** — the builder
  must be discoverable in `nixosConfigurations` (discovery/select); raw
  SSH-URL-only build servers are out of scope.
- **Passphrase-protected SSH keys / non-interactive daemon build auth** —
  forwarded to Nix's documented remote-build requirement (keys in
  `~/.ssh/authorized_keys`, `trusted-users`); no new auth capability.
- **Proxy-jump / advanced Nix `--builders` forms** (system-features,
  `ssh://…?ssh-key=…`) — only the simple `ssh://user@port` URI is emitted.
- **`.nod.toml` `[hosts.<name>.build.build_host]` as a lower tier** — the
  cascade lower tier is the flake `config.nod.build.buildHost` (which the
  `HostEntity` already materializes); TOML build-section defaults remain
  out of scope.

---

## Out of scope (explicit)

- No change to transfer/activation semantics (build action is
  `DeploymentAction::Build` today: build + optional out-link, never deploy).
- No new SSH/no new builder-adapter protocol beyond the single `--builders`
  URI mapping in the infra evaluator adapter.
- No new CLI surface beyond `--builder <HOST>` on `nod build`.

---

## Sizing

- Can be implemented in one focused session (~2–4h): one domain VO, one
  field on `DeploymentOptions`, one optional parameter on `EvaluatorPort`,
  one URI-mapping helper + selector resolution in the build command, and
  updated mocks/call-sites (6 build_toplevel callers, 5 DeploymentOptions
  literals). Right-sized.