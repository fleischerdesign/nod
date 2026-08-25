# Specification: Developer & Inspection Tools (Eval, REPL, Info)

## Status
Approved (ADR-016)

## Motivation
Equip developers and operators with tools to inspect NixOS configuration values, enter interactive REPL environments, and view comprehensive host diagnostic dashboards.

## Acceptance Criteria

### AC1: Expression Evaluation (`nod eval`)
- `nod eval <EXPR> [TARGET/GLOB]` must evaluate the specified expression in the context of the host's `config` (e.g. `services.nginx.enable` or `config.services.nginx.enable`).
- It must support targeting multiple hosts (e.g. `--all`, `--tag web`) and format results per host.
- When `--json` is provided, output must be valid JSON mapped per host.
- When `--raw` is provided, string outputs must omit surrounding quotes.

### AC2: Interactive REPL (`nod repl`)
- `nod repl [TARGET]` must resolve the target host (defaulting to the local host if omitted).
- It must launch `nix repl --impure` seeded with `{ flake, host, config, options, pkgs }`.
- Stdio must be connected interactively to the terminal.

### AC3: Host Diagnostic Dashboard (`nod info`)
- `nod info [TARGET/GLOB]` must resolve target hosts and gather:
  - Configuration attributes: host name, target host / IP, role, tags, builder, SSH profile.
  - Runtime attributes (if reachable): active profile generation, current system closure path, booted system closure path, kernel version, uptime, systemd health state.
- When `--json` is provided, it must output a structured array of `HostInfo` objects.

### AC4: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Format verified by `cargo fmt --check`.
