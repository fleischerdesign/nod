# nod — Unified Target Selection Specification

> Gherkin feature specification for the Unified Target Selector (ADR-006
> `006-unified-target-selection.md`). Each `Feature` is behavior-observable
> against **mock adapters** — no real Nix/SSH/network is required.
> The selector runs in the Application layer (`src/application/selection.rs`)
> over the discovered host list.

---

## Fixture: Fleet under test

```gherkin
@fixture @fleet
Feature: A stable fleet used by every selection scenario

  Scenario: the fixture fleet
    Given the discovered fleet:
      | name          | tags           | role      |
      | web-01        | prod, web      | server    |
      | web-02        | prod, web      | server    |
      | web-staging   | staging, web   | server    |
      | db-01         | prod, db       | server    |
      | db-prod-01    | prod, db       | server    |
      | edge-prod     | prod, edge     | notebook  |
    Then the fleet has 6 hosts
```

---

## Feature: Exact Name Match

```gherkin
@target @exact
Feature: A bare positional target resolves by exact hostname

  Scenario: an exact name selects exactly that host
    Given the fixture fleet
    When `nod status web-01` resolves its target
    Then the selected set is exactly [web-01]

  Scenario: an exact name wins over a matching glob
    Given the fixture fleet
    When `nod status web-01` resolves with a glob pattern that also matches
    Then the exact host web-01 is selected, never the glob's wider set

  Scenario: unknown exact names match nothing
    Given the fixture fleet
    When `nod status nowhere-01` resolves its target
    Then the selected set is empty
```

---

## Feature: Glob Pattern Matching

```gherkin
@target @glob
Feature: A pattern target matches hostnames by shell-style glob

  Scenario: prefix glob web-*
    Given the fixture fleet
    When `nod status "web-*"` resolves its target
    Then the selected set is [web-01, web-02, web-staging]

  Scenario: suffix glob *-prod
    Given the fixture fleet
    When `nod status "*-prod"` resolves its target
    Then the selected set is [db-prod-01, edge-prod]

  Scenario: infix glob *db*
    Given the fixture fleet
    When `nod status "*db*"` resolves its target
    Then the selected set is [db-01, db-prod-01]

  Scenario: a single-character wildcard narrows a glob
    Given the fixture fleet
    When `nod status "web-0?"` resolves its target
    Then the selected set is [web-01, web-02]

  Scenario: a malformed glob is a clean error
    Given the fixture fleet
    When `nod status "web-["` resolves its target
    Then the command fails with a target-pattern error
    And no host is selected
```

---

## Feature: Tag Filter

```gherkin
@target @tag
Feature: --tag narrows the candidate set to hosts holding the tag

  Scenario: tag filter over the whole fleet
    Given the fixture fleet
    When `nod status --all --tag prod` resolves its target
    Then the selected set is [web-01, web-02, db-01, db-prod-01, edge-prod]

  Scenario: a tag that no host holds matches nothing
    Given the fixture fleet
    When `nod status --all --tag database` resolves its target
    Then the selected set is empty
```

---

## Feature: Role Filter

```gherkin
@target @role
Feature: --role narrows the candidate set to hosts with the role

  Scenario: role filter over the whole fleet
    Given the fixture fleet
    When `nod status --all --role server` resolves its target
    Then the selected set is [web-01, web-02, web-staging, db-01, db-prod-01]

  Scenario: role filter combined with a glob
    Given the fixture fleet
    When `nod status "db-*" --role server` resolves its target
    Then the selected set is [db-01, db-prod-01]
```

---

## Feature: Boolean AND Intersection

```gherkin
@target @intersection
Feature: Multiple criteria are combined by boolean AND

  Scenario: glob AND tag AND role
    Given the fixture fleet
    When `nod exec "web-*" --tag prod --role server` resolves its target
    Then the selected set is [web-01, web-02]
    And web-staging is excluded (tag `staging`, not `prod`)

  Scenario: a contradictory intersection matches nothing
    Given the fixture fleet
    When `nod status "web-*" --tag db` resolves its target
    Then the selected set is empty
    And the error names the active criteria

  Scenario: selection is order independent
    Given the fixture fleet
    When `nod exec --role server --tag prod "web-*"` resolves its target
    Then the selected set is [web-01, web-02]
```

---

## Feature: Universal --all Flag

```gherkin
@target @all
Feature: --all addresses the whole fleet and may be narrowed

  Scenario: --all alone selects every discovered host
    Given the fixture fleet
    When `nod status --all` resolves its target
    Then the selected set is [web-01, web-02, web-staging, db-01, db-prod-01, edge-prod]

  Scenario: --all composes with filters
    Given the fixture fleet
    When `nod check --all --role notebook` resolves its target
    Then the selected set is [edge-prod]

  Scenario: --all and the all sentinel are equivalent
    Given the fixture fleet
    When `nod status all` resolves its target
    Then the selected set equals the set of `nod status --all`
```

---

## Feature: Error Handling — No Matching Hosts

```gherkin
@target @error
Feature: An empty selection fails cleanly and names the criteria

  Scenario: a pattern with no matches
    Given the fixture fleet
    When `nod status "nonexistent-*"` resolves its target
    Then the command fails with `no hosts matched`
    And the error message includes the target `nonexistent-*`

  Scenario: a combined filter with no matches
    Given the fixture fleet
    When `nod status "web-*" --tag prod --role notebook` resolves its target
    Then the command fails with `no hosts matched`
    And the error message lists the tag and role criteria

  Scenario: an empty fleet selection is still an error
    Given an empty discovered fleet
    When `nod status --all` resolves its target
    Then the command fails with `no hosts matched`
```

---

## Feature: Single-Target Restriction (Interactive Commands)

```gherkin
@target @single-host @interactive
Feature: ssh and repl require exactly one resolved host

  Scenario: an exact name resolves for ssh
    Given the fixture fleet
    When `nod ssh web-01` resolves its target
    Then the resolved target has exactly 1 host
    And the command opens a terminal on web-01

  Scenario: a glob resolving to several hosts is rejected for ssh
    Given the fixture fleet
    When `nod ssh "web-*"` resolves its target
    Then the command fails with a `multiple hosts matched` error
    And the error lists [web-01, web-02, web-staging]

  Scenario: no matches are rejected for repl
    Given the fixture fleet
    When `nod repl "nowhere-*"` resolves its target
    Then the command fails with `no hosts matched`

  Scenario: multi-host commands are not restricted
    Given the fixture fleet
    When `nod switch "web-*"` resolves its target
    Then the resolved target has 3 hosts
    And the command proceeds across the whole set
```

---

> **Conventions:** the selector is a pure function over the discovered host list:
> exact name match first, then shell-style glob, then `--tag`/`--role` filters,
> all combined with boolean AND; `--all` is the empty (identity) selector and may
> be narrowed. `ssh`/`repl` enforce the single-host invariant (0 or >1 matches is
> a clean error). Tag names are stable anchors for the runner.