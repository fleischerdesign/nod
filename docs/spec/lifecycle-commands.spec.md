# nod — Lifecycle Commands Specification

> Gherkin feature specification for the `nod test`, `nod boot` and
> `nod build` lifecycle commands (ADR-006 lifecycle commands, ADR-005 fleet
> concurrency, ADR-003 state machine). Each `Feature` is behavior-observable
> against **mock adapters** — no real Nix/SSH/network is required. The
> deployer seam (`DeployerPort::deploy_and_activate`) receives the concrete
> switch-to-configuration subcommand (`switch` / `test` / `boot`) as its
> `action` argument; `build` never reaches the deployer.

---

## Feature: `nod test` — switch-to-configuration test

```gherkin
@lifecycle @test-command
Feature: `nod test` runs switch-to-configuration test on the targeted fleet

  Scenario: a single named host is tested
    Given the fleet contains host atlas
    When the user runs `nod test atlas`
    Then the resolved deployer for atlas receives `deploy_and_activate` with action "test"
    And no host is marked failed

  Scenario: the fleet is narrowed by glob, tag and role
    Given the fleet contains web-01, web-02 and db-01
    When the user runs `nod test "web-*" --tag prod --role server`
    Then exactly web-01 and web-02 receive the "test" action
    And db-01 receives no deployer call

  Scenario: --all tests every discovered host
    Given a fleet of 6 hosts
    When the user runs `nod test --all`
    Then every host receives `deploy_and_activate` with action "test"

  Scenario: an unmatched target fails cleanly
    Given a fleet with no host named "nowhere"
    When the user runs `nod test nowhere`
    Then the run fails with a "no hosts matched" error naming the target

  Scenario: test results roll up into the fleet summary
    When `DeployFleetUseCase` runs with `DeploymentAction::Test`
    And every host's "test" activation succeeds
    Then every outcome is `Completed`
    And the summary reports zero failures
```

---

## Feature: `nod boot` — switch-to-configuration boot

```gherkin
@lifecycle @boot-command
Feature: `nod boot` runs switch-to-configuration boot on the targeted fleet

  Scenario: a single named host is booted
    Given the fleet contains host atlas
    When the user runs `nod boot atlas`
    Then the resolved deployer for atlas receives `deploy_and_activate` with action "boot"

  Scenario: `nod boot` honours target, tag, role and --all
    When the user runs `nod boot --all --tag prod`
    Then every prod-tagged host receives the "boot" action
    And every other host receives no deployer call

  Scenario: boot failures surface per host
    Given a host whose `deploy_and_activate` with action "boot" fails
    When `DeployFleetUseCase` runs with `DeploymentAction::Boot`
    Then that host is marked failed
    And with `--auto-rollback` the resolved deployer is asked to `rollback`
```

---

## Feature: `nod build` — build closures without transferring

```gherkin
@lifecycle @build-command
Feature: `nod build` evaluates `system.build.toplevel` and never transfers it

  Scenario: plain build evaluates and builds the toplevel closure
    Given a host targeted by `nod build atlas`
    When `DeployFleetUseCase` runs with `DeploymentAction::Build`
    Then the evaluator receives `build_toplevel` for atlas
    And the host outcome is staged
    And the resolved deployer receives no `deploy_and_activate` call

  Scenario: --out-link creates a symlink to the built closure
    Given the evaluator builds the closure at /nix/store/aaa-toplevel
    When the user runs `nod build atlas --out-link /tmp/result`
    Then /tmp/result is a symlink pointing at /nix/store/aaa-toplevel
    And no transfer or activation is attempted

  Scenario: the build action keeps the target/filter grammar
    When the user runs `nod build "db-*" --tag prod --role server`
    Then only the matching hosts are built

  Scenario: a failed evaluation marks the host failed
    Given the evaluator reports a build failure for host atlas
    When `DeployFleetUseCase` runs with `DeploymentAction::Build`
    Then the atlas outcome is `Failed`
    And no deployer call is made
```

---

## Feature: Concurrency Control and Multi-Target Selection

```gherkin
@lifecycle @concurrency @selection
Feature: lifecycle commands share the ADR-005 concurrency and ADR-006 selection policy

  Scenario: concurrency bounds in-flight lifecycle hosts
    Given `--concurrency 2` with 6 targeted hosts
    When the lifecycle run deploys them
    Then never more than 2 hosts are in flight at one instant

  Scenario: zero concurrency is rejected
    Given a run with `--concurrency 0`
    Then the command fails with "--concurrency must be at least 1"

  Scenario: `--all` is the identity selector, composable with filters
    When the user runs `nod test --all --tag edge`
    Then exactly the edge-tagged hosts are selected
    When the user runs `nod build "web-*" --role server`
    Then exactly the server-role web hosts are selected

  Scenario: an empty selection fails before any port call
    Given a fleet with no matching hosts
    When a lifecycle command runs
    Then the command fails with a no-hosts-matched error
    And the evaluator and deployers receive no per-host lifecycle call
```

---

## Feature: Error Handling

```gherkin
@lifecycle @error-mode
Feature: lifecycle commands fail loudly without partial activation surprises

  Scenario: build never activates a partial system
    Given `nod build --all` over a fleet
    When every closure builds
    Then no `switch-to-configuration` subcommand is executed

  Scenario: activation failure reporting
    Given a host whose "test" activation fails
    When `--auto-rollback` is set
    Then the host leaves the deployer path via explicit rollback
    And the outcome is `RolledBack` or `Failed`

  Scenario: fail-fast aborts remaining waves
    Given a `--fail-fast` lifecycle run over multiple hosts
    When the first wave contains a failure
    Then no later wave starts
    And the run summary is `aborted`
```

---

> **Conventions:** the lifecycle actions map to `DeploymentAction::{Test, Boot,
> Build}` in `domain/plan.rs`; the deployer action string is derived from
> `DeploymentAction::to_str`. Fleet policy (waves, semaphore, fail-fast) lives
> in `application/use_cases/deploy_fleet.rs` + `domain/plan.rs`; the commands
> parse `[TARGET/GLOB] [--tag] [--role] [--all]` through
> `TargetSelection::select` exactly like every host-operating command
> (ADR-006).