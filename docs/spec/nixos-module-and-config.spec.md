# nod — NixOS Module & Configuration Parity Specification

> Gherkin feature specification for the 1:1 correspondence between:
> the `options.nod` surface of `modules/nixos/default.nix`, the `config.nod`
> value emitted during Flake evaluation, the `.nod.toml` keys, and the Rust
> domain types in `src/domain/config.rs`. The module surface and the domain
> structs are mirrored so a value written in any tier round-trips losslessly
> into the other tiers (ADR-004). The `NixCliEvaluator` inspects hosts through
> the `EvaluatorPort` seam; implementers verify against mocked eval output —
> no real Nix/SSH/network is required.

---

## Feature: 1:1 Schema Parity Between Nix Module Options and TOML Keys

```gherkin
@parity @schema @nixos-toml
Feature: Every options.nod option has a deserializable peer in the domain model and TOML

  Scenario: SSH options pair across the module, TOML and the Rust profile
    Given the module declares nod.ssh.user with type str and default "root"
    And the module declares nod.ssh.port with type port and default 22
    And the module declares nod.ssh.timeoutSecs with type ints.positive and default 30
    When the SshProfileConfig domain struct is deserialized from JSON
    Then the field user maps to ssh.user
    And the field port maps to ssh.port
    And the field timeout_secs maps to ssh.timeoutSecs
    And the field connect_timeout_secs maps to ssh.connectTimeoutSecs
    And the field extra_ssh_args maps to ssh.extraSshArgs
    And the field allow_insecure maps to ssh.allowInsecure

  Scenario: build options map 1:1 to the module and TOML
    Given: the module declares nod.build.buildHost as nullOr str (default null)
    And the module declares nod.build.substituters as listOf str (default [])
    And the module declares nod.build.evalFlags as listOf str (default [])
    When the BuildConfig domain struct is deserialized
    Then the field buildHost maps to build.buildHost
    And the field substituters maps to build.substituters
    And the field trusted_public_keys maps to build.trustedPublicKeys
    And the field eval_flags maps to build.evalFlags
    And the field show_trace maps to build.showTrace
    And the field impure maps to build.impure

  Scenario: rollout and health-check groups pair across all three spellings
    Given: the module declares nod.rollout.priority as ints.unsigned (default 100)
    And the module declares nod.rollout.action as an enum of
      "switch" "boot" "test" "dry-run"
    And the module declares nod.healthChecks.systemd.requiredUnits as listOf str
    And the module declares nod.healthChecks.httpProbes as a list of submodules
    And the module declares nod.healthChecks.customProbes as a list of submodules
    When the domain structs RolloutConfig and HealthCheckConfig are deserialized
    Then the field auto_rollback maps to rollout.autoRollback
    And the field magic_rollback_timeout_secs maps to rollout.magicRollbackTimeoutSecs
    And the field health_checks maps to healthChecks
    And the field required_units maps to healthChecks.systemd.requiredUnits
    And the field http_probes maps to healthChecks.httpProbes (url + expectedStatus)
    And the field custom_probes maps to healthChecks.customProbes (name + command)

  Scenario: hooks options pair across the module and struct
    Given: the module declares nod.hooks.preSwitchHook as nullOr str (default null)
    When the HooksConfig domain struct is deserialized
    Then the field pre_switch_hook maps to hooks.preSwitchHook
    And the field post_switch_hook maps to hooks.postSwitchHook

  Scenario: scalar and enum options pair to their domain fields
    Given: the module declares nod.enable as bool (default false)
    And the module declares nod.role as an enum of six values
    And the module declares nod.description as nullOr str
    When the NodConfig domain struct is deserialized
    Then the field enable maps to nod.enable
    And the field role parses to the domain seam
    And the field description maps to nod.description
```

---

## Feature: Evaluation of `config.nod` from the Flake into Rust Domain Models

```gherkin
@evaluation @flake @nod-config
Feature: NixCliEvaluator materializes config.nod onto HostEntity.nod_config

  Scenario: the full module object deserializes into the host entity
    Given: a host whose flake emits a `nod` object with target "10.0.0.8"
    And ssh.user "philipp", ssh.port 2222
    And build.substituters ["https://cache.example"], rollout.action "test"
    And healthChecks.tcpPorts [443]
    When the host-metadata expression is evaluated
    Then the emitted JSON carries a non-null nod object
    And the deserialized HostEntity holds that object in nod_config
    And nod_config.target equals "10.0.0.8"
    And nod_config.ssh.port equals 2222
    And nod_config.build.substituters equals ["https://cache.example.nix"]

  Scenario: missing optional groups fall back to struct defaults
    Given: a flake whose nod object carries only enable and targetHost
    When the nod object is deserialized
    Then the domain struct defaults fill the ssh, build, rollout, health and
      hooks groups

  Scenario: a host with no nod module degrades gracefully
    Given: a flake with no nod attribute for a host
    When the evaluator discovers that host
    Then the nod field is absent
    And the HostEntity keeps the compiled defaults (root / 22)

  Scenario: per-host metadata falls back through deployment and networking
    Given: a flake declaring deployment.targetHost but no nod.targetHost
    And a flake declaring deployment.role "desktop"
    When the evaluator resolves per-host metadata
    Then the targetHost falls back to deployment.targetHost
    And the role falls back to deployment.role
```

---

## Feature: Merging Hierarchy (CLI > TOML > Nix Module > Defaults)

```gherkin
@merge @precedence @tiers
Feature: Options resolve with CLI > .nod.toml > Nix module > compiled defaults

  Scenario: The Nix module tier beats compiled defaults
    Given a flake declaring config.nod.ssh.port 2222 for a host
    And no TOML and no CLI override for that option
    When the effective profile for the host resolves
    Then the resolved SSH port is 2222
    And the resolved SSH user stays at the compiled default "root"

  Scenario: The TOML tier beats the Nix module tier
    Given: a flake declaring config.nod.ssh.user "nix-user"
    And a .nod.toml with [hosts.atlas] user "toml-user"
    When the effective profile for atlas resolves
    Then the resolved SSH user is "toml-user"

  Scenario: The CLI tier beats the TOML tier
    Given: a .nod.toml with [hosts.atlas] user "toml-user"
    When the user runs with --user "root"
    Then the resolved SSH user for atlas is "root"

  Scenario: options resolve independently per tier
    Given: a .nod.toml with [hosts.atlas] user "philipp"
    And a CLI override --port 2222
    When the effective profile for atlas resolves
    Then the resolved SSH user is "philipp"
    And the resolved SSH port is 2222

  Scenario: absent tiers leave the preceding tier untouched
    Given: no .nod.toml and no flake override
    When the effective profile for a remote host resolves
    Then the resolved SSH user is "root"
    And the resolved SSH port is 22
    And sudo is disabled for a remote host
```

---

> **Conventions:** the four tiers are merged per option with a fixed
> precedence (CLI > `.nod.toml` > Nix module `config.nod` > compiled
> defaults). Within the TOML tier, `[hosts.<name>]` beats `[fleet]` which
> beats `[defaults]`. The Nix module tier is exactly the `options.nod`
> surface declared by this flake's `modules/nixos/default.nix`.