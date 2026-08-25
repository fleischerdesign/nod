# ADR-016: Developer & Inspection Tools (Eval, REPL, Info)

## Context
Developers and operators need interactive and programmatically accessible tools to evaluate Nix expressions across fleet targets, enter REPL environments pre-loaded with host context, and view comprehensive diagnostic summaries:
1. `nod eval [EXPR] [TARGET/GLOB]`: Evaluate arbitrary Nix attribute paths (e.g. `config.services.nginx.enable`, `config.networking.firewall.allowedTCPPorts`) in the scope of targeted host configurations, supporting both human-readable and structured `--json` output.
2. `nod repl [TARGET]`: Launch an interactive `nix repl` session pre-seeded with `{ flake, host, config, options, pkgs }` for a specific host.
3. `nod info [TARGET/GLOB]`: Provide a rich inspection card aggregating static configuration metadata (roles, tags, builders, SSH profiles) and live system state (active generation, booted vs current system closure, kernel release, uptime, systemd health).

## Decision
1. **Domain Extensions**:
   - `EvalResult` (`host_name`, `expression`, `value`, `raw_output`).
   - `HostInfo` (`host_name`, `target_host`, `is_local`, `role`, `tags`, `ssh_user`, `ssh_port`, `current_closure`, `booted_closure`, `kernel_version`, `uptime`, `health_status`, `active_generation`, `builder`).
2. **Ports**:
   - `EvaluatorPort::eval_expr(flake_path: &Path, host_name: &str, expr: &str, json: bool) -> Result<String, NodError>`
3. **Application Use Cases**:
   - `EvalFleetUseCase`: Orchestrates expression evaluation across resolved targets.
   - `InspectInfoUseCase`: Collects and merges static configuration metadata with live system diagnostics per host.
4. **Presentation Layer**:
   - `src/commands/eval.rs` (`nod eval <EXPR> [TARGET/GLOB] [--tag] [--role] [--all] [--json] [--raw]`).
   - `src/commands/repl.rs` (`nod repl [TARGET]`).
   - `src/commands/info.rs` (`nod info [TARGET/GLOB] [--tag] [--role] [--all] [--json]`).

## Consequences
- **Positive**: Direct CLI introspection into deep NixOS module configurations across single nodes or entire fleet subsets.
- **Positive**: Zero-setup REPL debugging with top-level variable scope for host configs.
- **Positive**: Unified host dashboard combining static flake declarations with runtime host telemetry.
- **Positive**: Clean hexagonal boundaries with full unit test coverage and zero warning quality gates.
