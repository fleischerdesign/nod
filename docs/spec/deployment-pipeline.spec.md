# nod — Deployment Pipeline Specification

> Gherkin feature specification for the resilient deployment pipeline, rollback
> engine and fleet orchestration (ADR-003 `003-deployment-state-machine.md`,
> ADR-005 `005-fleet-concurrency.md`). Each `Feature` is behavior-observable
> against **mock adapters** — no real Nix/SSH/network is required.

---

## Feature: Deployment State Machine — Forward Progress

```gherkin
@pipeline @state-machine
Feature: A host advances through guarded, linear deployment stages

  Scenario: a healthy host deploys to completion
    Given a host prepared for deployment
    When the pipeline begins evaluation
    And evaluation reports Ok
    And build reports Ok
    And transfer reports Ok
    And switch reports Ok
    And verification reports Ok
    Then the host state is `Prepared`
    And the host advances through `Evaluating`, `Building`, `Transferring`,
      `Switching`, `Verifying`
    And the host ends in `Completed`

  Scenario: every advancing event is guarded
    Given a host in `Building`
    When the pipeline supplies the only legal successor event
    Then the host advances to the successor
    When the pipeline supplies any other event
    Then the transition is rejected as an invariant violation

  Scenario: a completed host refuses further transitions
    Given a host in `Completed`
    When the pipeline tries to advance it again
    Then the transition is rejected
```

---

## Feature: Failure and Auto-Rollback Transitions

```gherkin
@pipeline-state-machine @rollback
Feature: A failing host reverts through an explicit rollback stage

  Scenario: switch failure triggers rollback recovery
    Given a host in `Switching`
    When the switch step reports failure
    Then the host transitions to `RollbackTriggered`
    When the rollback completes successfully
    Then the host ends in `RolledBack`
    And the outcome is degraded, not `Completed`

  Scenario: verification failure triggers rollback recovery
    Given a host in `Verifying`
    When the health probe reports unhealthy
    Then the host transitions to `RollbackTriggered`
    When the rollback then fails
    Then the host ends in `Failed`

  Scenario: evaluation, build and transfer failures enter recovery
    Given a host in `Evaluating`, `Building` or `Transferring`
    When the active step reports failure
    Then the host transitions to `RollbackTriggered` and recovers

  Scenario: a failed rollback marks the host failed
    Given a host in `RollbackTriggered`
    When the rollback step reports failure
    Then the host transitions to `Failed`
    And the outcome is typed `NodError::deployment`

  Scenario: a host cannot jump from an active stage to a terminal state
    When the pipeline tries to roll back a host that never started switching
    Then the transition is rejected
```

---

## Feature: Plan Preview and Dry-Run

```gherkin
@pipeline-plan @dry-run
Feature: Plan preview builds closures and reports diffs without activating

  Scenario: generating a plan does not touch the live system
    Given a set of target hosts
    When `GeneratePlanUseCase` builds each toplevel closure
    Then each target records its `new_closure`
    And no `deploy_and_activate` call is made

  Scenario: dry-run deployment short-circuits before activation
    Given a `nod switch --dry-run` invocation
    When the fleet use case runs with `dry_run` set
    Then no switch command reaches any deployer
    And the plan is returned with every target staged
```

---

## Feature: Rollback Execution

```gherkin
@pipeline @rollback
Feature: `nod rollback` reverts a host to its previous generation

  Scenario: rollback dispatches through the resolved deployer
    Given a host that has no prior successful deployment
    When the rollback use case runs
    Then the resolved deployer for the target receives `rollback`
    And any failure returns a deployment typed error

  Scenario: a named host rollback targets exactly one host
    Given the CLI `nod rollback <host>`
    When the command resolves the target
    Then exactly the named host receives the rollback dispatch
```

---

## Feature: Fleet Concurrency Control

```gherkin
@fleet @concurrency
Feature: The fleet use case bounds in-flight hosts and applies a rollout strategy

  Scenario: All runs every host once
    Given a fleet use case with default options `All`
    When it runs `Deploy` over a fleet of 6 hosts
    Then every host has exactly one outcome
    And the total host deployments is 6

  Scenario: Canary verifies one host before the rest
    Given an `All-remaining` canary rollout of 6 hosts
    When the fleet use case runs
    Then the first host shades alone in its own wave
    And the remaining hosts deploy only after the canary outcome is observed

  Scenario: Batch rolls out fixed-size waves
    Given a `Batch` strategy with batch size 2 and 6 hosts
    When the fleet use case runs
    Then hosts deploy in three waves of two
    And a wave does not start until the previous wave completes

  Scenario: the semaphore bounds in-flight hosts
    Given a concurrency budget of topologically `--concurrency 2`
    When 6 hosts deploy under strategy `All`
    Then never more than 2 hosts are in flight at one instant
```

---

## Feature: Fleet Error Modes

```gherkin
@pipeline @fleet @error-mode
Feature: Fail-fast and continue-on-error govern the whole run

  Scenario: fail-fast aborts after the first failure
    Given a `fail_fast` run over 3 hosts
    When the first host fails verification
    Then no later wave starts
    And the run summary is `aborted`

  Scenario: continue-on-error isolates a failing host
    Given a `--continue-on-error` run over 3 hosts
    When the first host fails verification
    Then the failed host is marked degraded
    And the remaining hosts still deploy

  Scenario: auto-rollback restores a failed host
    Given a `--auto-rollback` switch failure
    When the host's switch step fails
    Then the resolved deployer is asked to `rollback`
    And the host ends `RolledBack` or `Failed`
```

---

> **Conventions:** each `Feature` maps to a cluster of fixtures. The state-machine
> transitions live in `application/pipeline/state_machine.rs`; fleet policy in
> `application/use_cases/deploy_fleet.rs` + `domain/plan.rs`; rollback is issued
> through the `DeployerPort::rollback` seam already resolved for that host.