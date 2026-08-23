# nod — Interactive Dashboard Specification

> Gherkin feature specification for the interactive Ratatui TUI dashboard (`nod dashboard`).
> The dashboard is the presentation seam into the same fleet model that `nod switch`, `nod diff`
> and `nod rollback` drive; it renders live host reachability plus per-host detail and operation
> logs. Verification targets `DashboardApp` state transitions and a `TestBackend`-rendered frame —
> no real terminal, Nix or network is required in tests.

---

## Feature: Dashboard Initialization

```gherkin
@dashboard @bootstrap
Feature: The dashboard starts from the discovered host fleet and a clean terminal

  Scenario: booting with a non-empty fleet
    Given Nix host discovery returns jello, atlas and orbit
    And terminal raw mode and the alternate screen are enabled
    When the dashboard event loop first runs
    Then the host matrix lists every discovered host
    And the first host is selected
    And the header shows the total host count and online/offline split
    And the footer presents the keybinding help

  Scenario: booting with zero hosts
    Given Nix host discovery returns no hosts
    When the dashboard initializes
    Then the loop exits cleanly without a terminal error
    And no selection index is produced

  Scenario: selection starts at a stable bound
    Given a fleet of jello, atlas and orbit
    When the model is constructed
    Then selected_index is 0
    And selected_host() returns jello
```

---

## 2. Host Matrix Navigation

```gherkin
@dashboard @navigation @Feature
Feature: The operator moves between hosts with j/k and the arrow keys

  Scenario: pressing Down/`j` advances the selection
    Given jello is selected
    When the operator presses Down (or `j`)
    Then atlas becomes selected

  Scenario: advancing past the last host wraps to the first
    Given orbit is selected
    When the operator presses Down (or `j`)
    Then jello is selected again

  Scenario: pressing Up/`k` moves the selection backwards
    Given orbit is selected
    When the operator presses Up (or `k`)
    Then atlas is selected

  Scenario: moving up past the first host wraps to the last
    Given jello is selected
    When the operator presses Up (or `k`)
    Then the last host orbit is selected

  Scenario: navigation does not move on an empty fleet
    Given there are no hosts
    When the operator presses Down or Up
    Then the selection index stays 0
```

---

## 3. View Switching

```gherkin
@dashboard @view @Feature
Feature: The operator switches the active pane with Tab

  Scenario: Tab cycles Matrix → Details → Logs
    Given the active pane is Matrix
    When the operator presses Tab
    Then the active pane is Details
    And pressing Tab again makes the active pane Logs
    And pressing Tab a third time returns the active pane to Matrix

  Scenario: the selected host persists across pane switches
    Given atlas is selected
    When the operator switches from Matrix to Details and back
    Then atlas is still selected
```

---

## 4. Action Triggers

```gherkin
@dashboard @actions @Feature
Feature: The dashboard triggers switch, rollback and diff operations

  Scenario: pressing `s` triggers a switch intent on the selected host
    Given jello is selected
    When the operator presses `s`
    Then a switch action is emitted
    And the status line notes the selected host

  Scenario: pressing `r` triggers a rollback intent
    Given orbit is selected
    When the operator presses `r`
    Then a rollback action is emitted for orbit

  Scenario: pressing `d` triggers a diff intent
    Given atlas is selected
    When the operator presses `d`
    Then a diff action is emitted for atlas

  Scenario: `q` requests a clean quit
    Given the dashboard is running
    When the operator presses `q`
    Then should_quit becomes true and the loop terminates
```

---

## 5. Clean Terminal Restoration on Exit

```gherkin
@dashboard @cleanup @Feature
Feature: Exiting the dashboard always restores the terminal

  Scenario: quitting through the loop restores the terminal
    Given the dashboard is running on the alternate screen
    When the operator quits
    Then raw mode is disabled
    And the alternate screen is left
    And the cursor is shown again

  Scenario: a panic restores the terminal
    Given the dashboard has installed a panic hook
    When a panic interrupts rendering
    Then raw mode is disabled and the alternate screen is left
    And the panic still surfaces to the default handler
```

---

> **Conventions:** the matrix and detail panes belong to the same `HostDashboardApp`
> state machine; reachability probes are asynchronous and offline hosts remain selectable.
> All rendering is backend-agnostic so tests can draw a frame into a `TestBackend` buffer
> without opening a real terminal.