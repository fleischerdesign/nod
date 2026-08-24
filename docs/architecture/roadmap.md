# nod — Architecture Roadmap

> **Status:** Foundation (target-state) — a forward-looking feature backlog for `nod` v2,
> tracked against the ADRs under [`../adr`](../adr) and the Gherkin specs under
> [`../spec`](../spec). Entries marked **✅ shipped** are implemented; the rest
> are *planned* capabilities, not yet shipped.

## Overview & Purpose

`nod` is the **Nix Orchestration & Deployment Engine v2**: it discovers
`nixosConfigurations`, evaluates them to store closures, transfers and switches
system configurations on local or remote machines (with or without bootloader
registration), verifies the result, and reports status. This roadmap orders the
standard *and* advanced NixOS command surface and feature themes the CLI/TUI must
gain, grouped by operational theme. Today only a fraction of that surface exists
(`switch`/`status`/`diff`/`check`/`plan`/`rollback`).

Every host-operating entry inherits the **Unified Target Selector** (ADR-006):
`[TARGET/GLOB]`, `--tag`, `--role`, and `--all` behave identically to `switch`,
narrowing by **boolean AND (set-intersection)** and staying order-free. Commands
with a single terminal (`ssh`, `repl`, `eval`) must resolve to **exactly one**
host — 0 or >1 matches is a clean typed error.

The roadmap builds on shipped wiring that is no longer a per-command concern:

- **Single composition root** — `AppContext::production(...)` in `main.rs` is the
  only place commands obtain the context, evaluator, both deployers, and the config
  store (ADR-008).
- **Effective connection profile** — the resolved `SshProfile` flows through the
  deploy port from the caller (ADR-007); `ssh`/`exec` bind a config store so
  configured `identity_file`/`proxy_jump`/port are honoured.
- **Centralized target resolution** — `resolve_targets` + `DefaultScope` in
  `application/selection.rs` is the single ADR-006 selection idiom used by every
  command (ADR-008).

## Theme 1 — Lifecycle & Deployment

The core activate/switch lifecycle: compile, transfer, and activate per-generation,
with bootloader registration split from the live service switch so the operator can
stage and validate before committing.

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod switch [TARGET/GLOB] [--tag] [--role] [--all]` | Build, copy, and **activate** — write the new generation to the store and register it as the boot default via the bootloader update (`switch-to-configuration`). This is the canonical `nod` operation. | P0 | Critical | Small | Existing: Application use case + transfer/switch ports + ADR-006 selector + ADR-003 state machine. |
| `nod test [TARGET/GLOB] [--tag] [--role] [--all]` | **Temporarily** activate without touching the bootloader: apply the closure for this session only (`switch-to-configuration test`), leaving the boot default untouched. | P1 | High | Small | App: reuses the switch path minus bootloader registration; new `test` verb on the deploy use case. |
| `nod boot [TARGET/GLOB] [--tag] [--role] [--all]` | Register the generation as the bootloader default **without** an immediate live service switch (`switch-to-configuration boot`) — the inverse of `test`. | P1 | High | Small | App: deploy use case writes only the boot entry; service activation deferred. |
| `nod build [TARGET/GLOB] [--tag] [--role] [--all] [--out-link L]` | Pure compilation of `system.build.toplevel` closures **without transfer or activation**; materialize an out-link at `L` when supplied. The local/preflight build path. | P0 | Critical | Small | App: build/link port; feeds transfer and cache export. |
| `nod plan [TARGET/GLOB] [--tag] [--role] [--all]` | Dry-run execution plan and closure diffing: evaluate the target closure set, diff against the deployed generation, and print the pending action step — without performing it. | P1 | High | Small | Existing `plan`; read-only App use case over discovery + store diff. |
| `nod rollback [TARGET]` | Revert to the previous generation (**exactly one** target — multi-match is a clean error; no `--generation` flag, generation-targeted rollback is deferred future work), re-arming the **auto-rollback** boundary so a failed activation recovers itself. | P1 | High | Small | Existing `RollbackUseCase`; ADR-003 state machine + history store; single-host via `select_exact_one`. |

## Theme 2 — Flake & Lockfile Management

Keep the input source of truth (the flake and its lockfile) consistent and verified
before building or deploying off it.

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod update [INPUTS...]` | Wrapper for `nix flake update`: refresh the input set, then print a **formatted package/commit diff summary** across the resolved inputs so a bump is reviewable before it is built on. | P1 | High | Medium | App: update use case wrapping the flake/nix port; presenter renders the diff. |
| `nod inputs` | Dump the **flake inputs tree**: resolved input names, URLs, commits/lock entries, derivations — the read-only inventory behind a given flake. | P2 | Medium | Small | Read-only query over the flake store; pairs with `metadata`. |
| `nod metadata` | Report **lockfile status**: rev count, dirty/pinned inputs, and the last-computed manifest checksum for the tracked flake. | P2 | Medium | Small | Read-only query over the metadata store. |
| `nod check [--strict]` | Quality-gate runner: format & lint + manifest validation (`nixfmt`, `deadnix`, `statix`, `nix flake check`); with `--strict`, promote warnings to failures. Existing check. | P0 | High | Small | Existing `CheckUseCase`; `--strict` flags the gate for CI. |

## Theme 3 — Generations & Store Management

Manage the store's installed generations: inventory, collect garbage, deduplicate,
and copy closures across hosts without altering the boot state.

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod generations [TARGET/GLOB] [--tag] [--role] [--all]` | List installed **remote/local** system generations per host: generation numbers, profiles, and activation history from the store directory. | P1 | High | Small | Read-only store query over the Nix store prefix; feeds `rollback`/`gc`. |
| `nod gc [TARGET/GLOB] [--tag] [--role] [--all] [--keep N] [--older-than X]` | Remote garbage collection: collect unused closures while **never collecting live closures**; keep the last `--keep N` generations and everything newer than `--older-than X`. | P2 | Medium | Medium | Infra: new `NixStorePort` seam; `--dry-run` default. |
| `nod store optimize [TARGET/GLOB] [--tag] [--role] [--all]` | Remote **hardlink deduplication** via `nix-store --optimise`: collapse identical store paths to one physical file. | P3 | Low | Medium | Infra: store adapter; read-only store walk. |
| `nod copy [TARGET/GLOB] [--tag] [--role] [--all] [--to/--from URL]` | **Targeted closure copy without activation**: transfer a specific (sub)set of closures to/from a host (`--to`/`--from`) before any switch, so the build is staged where it will be activated. | P1 | Medium | Medium | App/Action: reuses the transfer port with an explicit copy verb, not `switch`. |

## Theme 4 — Day-2 Operations & Administration

Interactive and bulk remote management: an established, deployed fleet.

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod ssh <TARGET>` | Open an interactive shell on **exactly one** host — 0 or >1 matches is a clean error. Uses the resolved `SshProfile` for secrets, not ad-hoc defaults. | P1 | High | Small | Presentation + Application; single-host invariant reuses the ADR-006 selector and an ADR-002 typed error. |
| `nod exec [TARGET/GLOB] [--tag] [--role] [--all] "<CMD>"` | **Parallel multi-host** remote runner: execute the quoted command across the resolved target set; quote the glob, `--` separates the remote command. | P1 | High | Medium | Application use case + Nix/SSH port; no new domain concept. |
| `nod eval <TARGET> <ATTRIBUTE>` | Evaluate an arbitrary NixOS option for the target **without building**: resolve the closure, read the attribute, and print it. Exactly-one target. | P2 | Medium | Medium | App use case over the evaluator; single-terminal invariant. |
| `nod repl <TARGET>` | Interactive `nix repl` with the **host's configuration preloaded**; reuse the normal selector with the exactly-1 invariant. | P2 | Medium | Medium | App: reuses the ADR-006 single-terminal invariant. |
| `nod reboot [TARGET/GLOB] [--tag] [--role] [--all] [--wait]` | **Coordinated rolling reboots**: drain, reboot, and (with `--wait`) poll reachability + post-reboot health checks — the ADR-005 rolling rollout shape. | P2 | High | Medium | App: reuses ADR-003 state machine + ADR-005 fleet rollout. |
| `nod dashboard` ✅ shipped | An interactive **Ratatui TUI**: streams the fleet status matrix, generation, and audit history in a terminal UI. | P1 | High | Small | Presentation layer over the existing status + audit use cases; no domain change. Action keys (`s`/`r`/`d`) now dispatch live single-host switch/rollback/diff on the selected host from the TUI event loop (see `src/ui/mod.rs::run_action`); a long-running action blocks the TUI until it completes (inline await — background rendering is future work). |

## Theme 5 — Secrets & Security

Key material and audit integrity: verify decryptability before ops, rotate the
fleet's recipients, and keep an append-only history of deployments and rollbacks.

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod secret check [TARGET/GLOB] [--tag] [--role] [--all]` | Pre-flight secret **decryptability** verification: every secret on the target must decrypt with the present/current sops or age key. | P1 | High | Medium | Infra: sops/age store adapter; failures surface as ADR-002 typed errors. |
| `nod secret rekey [TARGET/GLOB] [--tag] [--role] [--all]` | **Fleet-wide age recipient rotation**: rotate the recipient set and re-encrypt all secrets through the ADR-005 rollout plan. | P2 | High | Medium | Infra: same store adapter; a rolling operation. |
| `nod audit [TARGET] [--limit N]` ✅ shipped | **Append-only** deployment & rollback audit log per host (renamed from `nod history`; existing `AuditLogUseCase`); cap the rendered window with `--limit N`. | P2 | Medium | Small | Application use case + audit store; rendered in the TUI matrix. |

## Theme 6 — Day-0 Provisioning & Scaffolding

Bootstrap a bare machine or scaffold a fresh flake from the repo's own configuration.

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod bootstrap <TARGET> --ip <IP> [--disko]` | End-to-end **bare-metal installer from live ISO**: partition (`--disko`) and install NixOS via **nixos-anywhere** from the repo's own config — no manual live env. | P1 | High | Large | Infra: disko/anywhere adapters behind a `ProvisioningPort`; the flake stays the single source of truth. |
| `nod init [--template ...]` | Scaffold a new flake repository: layout, `nod.nixosModules.default` wiring, and default host, optionally from a `--template`. | P2 | Medium | Medium | App: template engine + file-writer port; one-time action, no daemon. |
| `nod iso [TARGET/GLOB] [--tag] [--role] [--all]` | Generate a custom **bootable installer ISO** for the target. | P3 | Medium | Large | Infra: adapter around `nixos-generators`; rarely on the critical path. |

## Theme 7 — Build & Cache Optimization

| Command | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod cache push [TARGET/GLOB] [--tag] [--role] [--all] [--cache URL]` | Push already-built closures to a **binary cache** (cache.nixos.org or a self-hosted substituter). | P1 | High | Medium | Infra: binary-cache adapter; the build produces closures, this exports them. |
| `nod build --builder <HOST>` ✅ shipped | Through a builder host: an explicit `--builder <host>` for distributed remote compilation (flake `config.nod.build.buildHost` as the lower cascade tier) — single-host build, no activation. | P1 | High | Medium | App: builder selection through the build port; fits the ADR-005 one-host-at-a-time shape. |

## Theme 8 — GitOps & Developer Experience

| Feature | Behavior | Priority | Impact | Effort | Architectural fit |
|---|---|---|---|---|---|
| `nod watch [TARGET]` | **Live auto-preview**: rebuild and closure-diff a target on every file save, streaming the result. | P1 | High | Small | App: file watcher over the plan/build path; presenter streams output. |
| `nod info [TARGET]` | A comprehensive **hardware & system profile** summary: role, tags, closure, CPU/RAM/disks, last history entry. | P2 | Medium | Small | Read-only query over discovery + store + hardware probe. |
| `nod graph [--format svg\|mermaid\|dot]` | **Flake topology and dependency graph**: hosts, roles, tags, groups, and input dependencies rendered in the chosen format. | P3 | Low | Medium | App: graph query port; feeds the TUI and CI dashboards. |
| `nod sync` / `nod daemon` | A pull-based **GitOps background reconciler** watches the repo (or a registry) and applies the changed flake with a rollout strategy. | P1 | High | Large | App: watcher service + ADR-005 rollout controller; each host's state machine reaches `Completed`, then stops. |
| `nod export <FORMAT>` | Export the resolved fleet to **Ansible Inventory / Prometheus Discovery** (and flake JSON / lifecycle snapshots / docs) for external tooling. | P2 | Medium | Medium | App: exporter port; pairs with `nod graph` and CI artifacts. |

---

## Updated Impact-vs-Effort matrix

The themes above consolidate to one shared planning view:

| Effort | Small | Medium | Large |
|---|---|---|---|
| High impact | `nod test`, `nod boot`, `nod plan`, `nod watch`, `nod audit`, `nod dashboard` | `nod exec`, `nod update`, `nod secret check`, `nod cache push`, `nod reboot` | `nod bootstrap`, `nod sync`/`daemon` |
| Medium impact | `nod build`, `nod generations`, `nod info`, `nod rollback` | `nod eval`, `nod secret rekey`, `nod inputs`/`metadata`, `nod check` | `nod build --builder`, `nod iso`, `nod store optimize` |
| Low impact | — | `nod copy`, `nod graph`, `nod export` | — |

**Suggested order (by value):** ship the zero-cost wins first — the shipped core
(`switch`/`build`/`plan`/`check`/`rollback`) proves the selector & store seams, and the
existing `AuditLogUseCase`, `watch` (existing status), and `dashboard` (existing status)
come next. Then the P1 selector-heavy trio `exec`/`secret check`/`update`, `test` and
`boot` (small verbs over the swapp path), and the provisioning/GitOps giants
(`bootstrap`, `sync`/`daemon`) last, once the Application/Infra seams above are proven.

## Architectural fit summary

| Area | Layer | Ports / use cases it touches | ADR anchors |
|---|---|---|---|
| 1. Lifecycle/deploy | App + Infra | transfer/switch/bootloader ports; deploy, plan, rollback use cases | ADR-006, ADR-003, ADR-002 |
| 2. Flake/lockfile | App + Infra | flake/update, store, quality-gate adapters | ADR-006 |
| 3. Generations/store | Infra | new `NixStorePort` seam; store walker | ADR-006, ADR-001 |
| 4. Day-2 ops | Presentation (TUI) + App | selector + SSH port + rollout controller | ADR-006, ADR-002, ADR-005, ADR-007, ADR-008 |
| 5. Secrets | Infra | sops/age adapters; typed errors; rolling rollout | ADR-002, ADR-005 |
| 6. Provisioning | Infra | disko/anywhere/ISO adapters (`ProvisioningPort`) | ADR-001 (ports) |
| 7. Build/cache | Infra | cache adapter + builder selection | ADR-005, ADR-007, ADR-008 |
| 8. GitOps/DX | App daemon + App | watcher + rollout controller + exporter + graph queries | ADR-005, ADR-003 |

## Definition of Done for roadmap entries

- The behavior is pinned by a Gherkin scenario in the matching spec file under `../spec`.
- The port/use-case seam it needs lands with mock-adapter unit tests first (ADR-001).
- Failure paths are typed errors (ADR-002), not ad-hoc `anyhow!` strings.
- The unified target grammar — every host-operating command accepts the full
  ADR-006 grammar (`[TARGET/GLOB]`, `--tag`, `--role`, `--all`) and behaves
  identically to `switch`; single-terminal commands resolve to exactly one host.
- Where a verb builds on `switch` (e.g., `test`, `boot`, `rollback`), the common switch/deploy use case is shared — not a copy of its logic.
- Impact/effort is unchanged unless review proves otherwise; a claimed priority is
  not shipped until the above hold.

> **Status:** Foundation — entries marked ✅ shipped are shipped; each other
> entry is a planned capability, not yet shipped. Everything above is slated for
> `nod v2` milestones in the order suggested by the impact-vs-effort matrix; this
> file is the single tracking document for the roadmap.