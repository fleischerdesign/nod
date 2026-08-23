# nod — Architecture Roadmap

> **Status:** Foundation (target-state) — a forward-looking feature backlog for `nod` v2,
> tracked against the ADRs under [`../adr`](../adr) and the Gherkin specs under
> [`../spec`](../spec). Every entry is a *planned* capability, not yet shipped.

## Purpose

`nod` is the **Nix Orchestration & Deployment Engine**: discover `nixosConfigurations`,
evaluate them to store closures, transfer and switch configurations on local or remote
machines, verify the result, and report status. Today a fraction of that surface exists
(`switch`/`status`/`diff`/`check`/`plan`/`rollback`). This roadmap orders the
capabilities the CLI/TUI must gain, grouped by operational theme.

Every host-operating entry inherits the **Unified Target Selector** (ADR-006):
`[TARGET/GLOB]`, `--tag`, `--role`, and `--all` behave identically to `switch`.

## Theme 1 — Day-2 Operations

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod ssh` | Open an interactive shell on **exactly one** host — 0 or >1 matches is a clean error. | P1 | High | Small | Presentation + Application; the single-host invariant reuses the ADR-006 selector and an ADR-002 typed error. |
| `nod exec` | Run an arbitrary command across the resolved target set (quote the glob; `--` separates the remote command). | P1 | High | Medium | Application use case + Nix/SSH port; no new domain concept. |
| `nod gc` | Run the Nix garbage collector on the target(s); NixOS needs care (never collect live closures). | P2 | Medium | Medium | Infra: new `NixStorePort` seam with `--dry-run` default. |
| `nod reboot` | Reboot resolved target(s) after a configurable drain delay, then wait for reachability. | P2 | High | Medium | Reuses the ADR-003 state machine + ADR-005 fleet rollout. |

## Theme 2 — Day-0 Provisioning

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod bootstrap` | Install NixOS from the repo's own config via **disko** partitioning + **nixos-anywhere**. | P1 | High | Large | Infra: `disko`/`nixos-anywhere` adapters behind a provisioning port; the flake stays the single source of truth. |
| `nod init` | Scaffold a new flake repository with the standard layout, module wiring, and a default host. | P2 | Medium | Medium | App: template engine + file-writer port; one-time action, no daemon. |
| `nod iso` | Build a bootable ISO image from the flake. | P3 | Medium | Large | Infra: adapter around `nixos-generators`; lowest urgency — rarely on the critical path. |

## Theme 3 — Secrets & Security (`nod secret`, `nod audit`)

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod secret check` | Verify every secret on the target is decryptable (age/sops key present and current). | P1 | High | Medium | Infra: sops/age store adapter; failures surface as ADR-002 typed errors reported per host. |
| `nod secret rekey` | Rotate the age recipients for a host or fleet; re-encrypt all secrets. | P1 | High | Medium | Same store adapter; a rolling operation — runs through the ADR-005 rollout plan. |
| `nod audit` | Append-only deployment/rollback history per host (existing `AuditLogUseCase`). | P2 | Medium | Small | Application use case + history store; rendered in the TUI matrix. |

## Theme 4 — Developer & Operator Experience (`nod watch`, `nod repl`, `nod info`, `nod graph`)

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod watch` | Stream the fleet status matrix (live updates) instead of a one-shot `status`. | P1 | High | Small | Presentation: Ratatui dashboard; polls the same `StatusUseCase` — no domain change. |
| `nod repl` | Interactive mode against the fleet using the normal selector with the **exactly-1** invariant. | P2 | Medium | Medium | App: reuses the single-terminal invariant of ADR-006. |
| `nod info` | Print a host's profile (tags, role, closure, last history entry). | P2 | Medium | Small | Read-only query over discovery + history store. |
| `nod graph` | Emit a relation graph of hosts (roles, tags, groups) in DOT/JSON. | P3 | Low | Medium | App: graph query port; feeds the TUI and CI dashboards. |

## Theme 5 — Build & Cache (`nod cache push`, `nod build --builder`)

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod cache push` | Push built closures to a binary cache (cache.nixos.org or a self-hosted substituter). | P1 | High | Medium | Infra: binary-cache adapter; the build already produces closures, this exports them. |
| `nod build --builder` | Build through a builder host (the flake's builders or `--builder <host>`). | P1 | High | Medium | App: builder selection; single-host build through the existing build port — fits the ADR-005 shape (one host at a time) without activation. |

## Theme 6 — GitOps (`nod sync` / `nod daemon`, `nod export`)

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod sync` / `nod daemon` | A background agent watches the repo (or a registry) and applies the changed flake with a rollout strategy. | P1 | High | Large | App: watcher service + ADR-005 rollout controller; each host's state machine reaches `Completed` and the agent stops. |
| `nod export` | Render the resolved state as portable reviewable artifacts (flake JSON, lifecycle snapshots, docs). | P2 | Medium | Medium | App: exporter port; pairs with `nod graph` and CI artifacts. |

---

## Impact vs effort

| Effort | Small | Medium | Large |
|---|---|---|---|
| High impact | `nod watch`, `nod audit`, `nod ssh` | `nod exec`, `nod secret check`, `nod secret rekey`, `nod cache push` | `nod bootstrap`, `nod sync`/`daemon` |
| Medium impact | `nod info` | `nod reboot`, `nod gc`, `nod init`, `nod export`, `nod repl` | `nod build --builder`, `nod iso` |
| Low impact | — | `nod graph` | — |

**Suggested order (by value):** ship the zero-cost wins first — `nod audit`
(existing `AuditLogUseCase`), `nod watch` (existing status), `nod ssh` (selector +
one invariant) — then the P1 selector-heavy trio `nod exec` / `nod secret check` /
`nod cache push`, and `bootstrap`/`daemon` last, once the Application/Infra seams
above are proven.

## Architectural fit summary

| Area | Layer | Ports / use cases it touches | ADR anchors |
|---|---|---|---|
| 1. Day-2 ops | Infra + App | Nix CLI adapter, SSH deployer, new `NixStorePort` seam | ADR-006, ADR-002 |
| 2. Provisioning | Infra (disko/anywhere/ISO adapters) | new `ProvisioningPort` | ADR-001 (ports) |
| 3. Secrets | Infra (sops/age adapter) | typed errors; rolling rollout under canary | ADR-002, ADR-005 |
| 4. DX | Presentation (TUI) + App | read-only queries | ADR-006 (repl invariant) |
| 5. Build/cache | Infra (cache adapter + builder) | build export | ADR-005 |
| 6. GitOps | App daemon service | watcher + rollout controller | ADR-005, ADR-003 |

## Definition of Done for roadmap entries

- The behavior is pinned by a Gherkin scenario in the matching spec file under `../spec`.
- The port/use-case seam it needs lands with mock-adapter unit tests first (ADR-001).
- Failure paths are typed errors (ADR-002), not ad-hoc `anyhow!` strings.
- The command accepts the full ADR-006 target grammar and behaves identically to `switch`.

> **Status:** Foundation — each entry is a planned capability, not yet shipped.
> Everything above is slated for `nod v2` milestones in the order suggested by the
> impact-vs-effort matrix; this file is the single tracking document for the roadmap.