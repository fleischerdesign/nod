# nod — Configuration & Host-Management Specification

> Gherkin feature specification for the configuration and host-management engine
> (ADR-004: four-tier configuration hierarchy). Each `Feature` is behavior-observable
> through the `ConfigStorePort` / `TargetSelection` seams; implementers and the
> test-engine verify against **mock adapters and on-disk `.nod.toml` fixtures** —
> no real Nix/SSH/network is required.

---

## Feature: Four-Tier Configuration Precedence

```gherkin
@config @precedence
Feature: Options resolve with CLI > .nod.toml > flake metadata > compiled defaults

  Scenario: CLI flags are the highest tier
    Given a .nod.toml with [hosts.atlas] user "philipp"
    When the user runs with --user "root"
    Then the resolved SSH user for atlas is "root"
    And every other option still comes from the TOML tier

  Scenario: .nod.toml overrides flake metadata
    Given a flake declaring config.nod.user "nix-user" for host atlas
    And a .nod.toml with [hosts.atlas] user "toml-user"
    When the effective profile for atlas resolves
    Then the resolved SSH user is "toml-user"

  Scenario: flake metadata overrides compiled defaults
    Given a flake declaring config.nod.port 2222 for host atlas
    And no TOML and no CLI override for that option
    When the effective profile for atlas resolves
    Then the resolved SSH port is 2222
    And the resolved SSH user is the compiled default "root"

  Scenario: compiled defaults are the fallback
    Given no .nod.toml and no flake override
    When the effective profile for a remote host resolves
    Then the resolved SSH user is "root"
    And the resolved SSH port is 22
    And sudo is disabled for a remote host

  Scenario: options resolve independently per tier
    Given a .nod.toml with [hosts.atlas] user "philipp"
    And a CLI override --port 2222
    When the effective profile for atlas resolves
    Then the resolved SSH user is "philipp"
    And the resolved SSH port is 2222
```

---

## Feature: Default Flake Root

```gherkin
@precedence @flake
Feature: The effective flake root resolves as --flake > [defaults].flake > the /etc/nixos/flake.nix marker > .

  Scenario: explicit --flake beats the config default
    Given a .nod.toml with [defaults] flake "/srv/nixos"
    When the user runs a command with --flake "/tmp/other"
    Then the effective flake root is "/tmp/other"

  Scenario: [defaults].flake applies when no --flake is given
    Given a .nod.toml with [defaults] flake "/srv/nixos"
    When the user runs a command without --flake from a directory under that file
    Then the effective flake root is "/srv/nixos"

  Scenario: a relative [defaults].flake resolves against the config file location
    Given a .nod.toml at /srv/nixos/.nod.toml with [defaults] flake "flakes/web"
    When no --flake is given
    Then the effective flake root is "/srv/nixos/flakes/web"

  Scenario: Marker beats cwd
    Given no .nod.toml with a flake key anywhere above the cwd
    And a flake.nix existing at /etc/nixos
    When the user runs a command without --flake from any directory
    Then the effective flake root is "/etc/nixos"

  Scenario: CLI beats marker
    Given no .nod.toml with a flake key anywhere above the cwd
    And a flake.nix existing at /etc/nixos
    When the user runs a command with --flake "/tmp/other"
    Then the effective flake root is "/tmp/other"

  Scenario: Working-dir fallback
    Given no .nod.toml with a flake key anywhere above the cwd
    And no /etc/nixos/flake.nix
    When the user runs a command without --flake
    Then the effective flake root is "."
```

---

## Feature: Host Selection by Tag, Role or Name

```gherkin
@selection @fleet
Feature: TargetSelection narrows the fleet by name, tag or role

  Scenario: selecting a host by name
    Given a fleet of jello, atlas and orbit
    When the target is "atlas" with no tag or role filter
    Then exactly the host named atlas is selected

  Scenario: local target resolves through hostname and locality
    Given a fleet containing the local host
    When the target is "local"
    Then the host matching the local hostname or marked local is selected

  Scenario: filtering by tag
    Given a fleet where atlas and orbit carry the tag "prod"
    When the target is "all" with --tag prod
    Then exactly atlas and orbit are selected

  Scenario: filtering by role
    Given a fleet where jello has role desktop and atlas/orbit have role server
    When the target is "all" with --role server
    Then exactly atlas and orbit are selected

  Scenario: tag and role filters combine
    Given a fleet where only orbit is tagged "edge"
    When the target is "all" with --tag edge --role server
    Then exactly orbit is selected
    And a tag/role pair with no matching host selects nothing

  Scenario: filters apply on top of name and local targets
    Given a fleet where only atlas is tagged "prod"
    When the target is "atlas" with --tag prod
    Then atlas is selected
    And the same target with --tag dev selects no host

  Scenario: no match produces a typed error
    Given a fleet with no host tagged "dev"
    When a run selects --tag dev
    Then the command fails with NodError::config
    And the message mentions the target and the active filters
```

---

## Feature: SSH Profile Resolution

```gherkin
@ssh @profile
Feature: The effective SshProfile merges user, port, identity file, proxy jump and sudo

  Scenario: user and port fall through defaults → fleet → host
    Given a .nod.toml with [defaults] user "deploy"
    And a .nod.toml with [fleet] port 2200
    And no per-host TOML for jello
    When the effective profile for jello resolves
    Then the SSH user is "deploy"
    And the SSH port is 2200

  Scenario: per-host settings beat fleet and defaults
    Given a .nod.toml with [defaults] user "deploy" and [fleet] user "fleet"
    And a .nod.toml with [hosts.atlas] user "philipp" port 2222
    When the effective profile for atlas resolves
    Then the SSH user is "philipp"
    And the SSH port is 2222
    And a host without its own section still resolves "fleet"

  Scenario: identity file resolution
    Given a .nod.toml with [fleet] identity_file "~/.ssh/fleet_key"
    And a .nod.toml with [hosts.atlas] identity_file "~/.ssh/atlas_key"
    When the effective profile for atlas resolves
    Then the identity file is "~/.ssh/atlas_key"
    And the identity file for jello is "~/.ssh/fleet_key"

  Scenario: proxy jump resolution
    Given a .nod.toml with [fleet] proxy_jump "bastion.example.org"
    When the effective profile for atlas resolves
    Then the proxy jump is "bastion.example.org"

  Scenario: sudo flag is merged independently
    Given a .nod.toml with [hosts.atlas] sudo true
    When the effective profile for atlas (a remote host) resolves
    Then the resolved profile requests sudo

  Scenario: CLI identity-file override beats TOML
    Given a .nod.toml with [hosts.atlas] identity_file "~/.ssh/toml_key"
    When the user runs with --identity-file "~/.ssh/cli_key"
    Then the resolved identity file is "~/.ssh/cli_key"

  Scenario: absent sections leave defaults untouched
    Given no .nod.toml for the flake
    When the effective profile for a remote host resolves
    Then the profile has no identity file and no proxy jump
    And the connection timeout stays at the profile default
```

---

> **Conventions:** tier values are merged per option with a fixed precedence
> (CLI > `.nod.toml` > flake metadata > compiled defaults); `[hosts.<name>]`
> sections beat `[fleet]` which beats `[defaults]` within the TOML tier.
> The **default flake root** uses its own cascade — CLI `--flake` >
> `[defaults].flake` (discovered from the cwd, relative values resolved
> against the config file's directory) > the system marker
> (`/etc/nixos/flake.nix`, the nixos-rebuild convention) > `.` — and stays
> out of `config.nod`. The marker is read on the control host where nod runs —
> the controller's `/etc/nixos/flake.nix`, not a target's (unlike
> `nixos-rebuild`, which reads the marker on the target) — so remote-fleet
> operators pin per-fleet roots with `--flake` or `[defaults].flake`.
> Tag names are stable anchors for the runner.
