# nod — Foundation Specification

> Gherkin feature specification for the foundation of the `nod` Hexagonal/Clean architecture transformation.
> Each `Feature` is behavior-observable without real Nix/SSH/network: implementers and test-engine verify against **mock adapters**.

---

## Feature: Domain Entity — `Host`

```gherkin
@domain @entity
Feature: A Host identifies a NixOS configurable target

  Scenario: constructing a local host
    Given a target user not otherwise specified
    When a Host is constructed with name "jello" and targetHost "jello-machine"
      And `is_local` is true
    Then the host's role is `server`
    And the host's target_user is "root"
    And the host's target_port is 22
    And the host has no active closure

  Scenario: constructing a remote host
    When a Host is constructed with name "atlas", targetHost "10.0.0.8", and is_local false
    Then the host is NOT local
    And the host's active closure is empty

  Scenario: role assignment round-tripping
    Given a host with role `desktop`
    When the role is serialized and deserialized
    Then the role survives unchanged
    And an unknown role string maps to the `unknown(String)` variant
```

---

## Feature: Domain Value Object — `SshProfile`

```gherkin
@domain @value-object
Feature: An SshProfile is an immutable connection descriptor

  Scenario: default profile when implicit
    Given a host with no explicit user or port
    When an SSH profile is derived for the host
    Then the profile user is "root"
    And the profile port is 22

  Scenario: explicit overrides are preserved
    Given a host with explicit user "philipp" and base 2222
    When an SSH profile is derived for the host
    Then the profile user is "philipp"
    And the profile port is 2222

  Scenario: immutability
    Given a derived SshProfile
    When code attempts to mutate a returned profile
    Then the mutation is rejected (connection values are copied, never changed in place)
```

---

## Feature: Typed Error Propagation

```gherkin
@errors @typed
Feature: Failures carry a structured NodError class across the boundary

  Scenario: evaluation failure surfaces as EvaluationError
    Given a mocked evaluator configured to fail discovery
    When the switch use case calls discover_hosts
    Then the command result is `NodError::evaluation`
    And the error message mentions the underlying cause

  Scenario: activation failure is typed as DeploymentError
    Given a mocked deployer configured to fail `deploy_and_activate`
    When the state machine exceeds the `switching` transition
    Then the host transitions to `ROLLBACK`
    And the aggregated run error is `NodError::deployment`

  Scenario: config isolation stays typed
    Given TOML that cannot be parsed
    When the configuration tier resolves
    Then a `NodError::config` is raised with the parse detail

  Scenario: health verification failure is typed
    Given a health probe that returns unhealthy
    When the `verifying` step runs
    Then the host moves to `ROLLBACK`
    And the error class is `NodError::healthcheck`
```

---

## Feature: Local vs Remote Deployment

```gherkin
@deployer @infrastructure
Feature: Hosts dispatch to the correct deployer by target

  Scenario: a local host uses the local deployer
    Given a host marked local
    And a LocalDeployer is registered for the expected host CLI
    When deployment resolves the deployer
    Then the local deployer starts `deploy_and_activate`
    And the adapter invokes `sudo switch-to-configuration` locally

  Scenario: a remote host uses the SShDeployer
    Given a remote host
    Then the SshDeployer resolves
    When deploy_and_activate runs
    Then the adapter performs a store copy and an SSH switch activation

  Scenario: reachability probe routing
    Given a local host
    When the health probe `check_reachability` runs
    The local adapter does not invoke the SSH transport
  Scenario: rollback resolves through the same deployer
    When a deployment fails
    Then rollback is issued through the same resolved deployer as the failed host
```

---

## Feature: AppContext DI Container

```gherkin
@appcontext @di
Feature: AppContext resolves ports and usecases from configuration

  Scenario: default resolution
    Given an AppContext seeded with a CLI and no custom registrations
    When the switch usecase resolves `evaluator` and `deployer`
    Then the resolved evaluator is the Nix CLI adapter
    And the resolved deployer is local or SSH matching the host's target

  Scenario: overriding a port in tests
    Given an AppContext whose evaluator port is a mock
    When the switch usecase runs
    Then the mock, not the real Nix CLI, is invoked
    And no real Nix commands are executed

  Scenario: unknown service is a config error
    When code asks the context for an unregistered service
    Then a `NodError::config` is raised listing the missing binding

  Scenario: config tiers are merged before use
    Given a seeded CLI flag and a TOML value
    When AppContext materializes the merged Config
    Then the higher-tiered (CLI) setting wins
```

---

> **Conventions:** each Scenario maps to one (or a small cluster of) test fixtures since mock adapters replace the Nix/SSH surfaces. Tag names (`@deployer`, `@errors`) are stable anchors for the runner.