# ADR-026: Universal Target Activation Contract (Heterogeneous & Agentless Target Orchestration)

## Status
Accepted (target-state for `nod v2.x`)

## Context
Historically, `nod` exclusively evaluated and deployed NixOS configurations (`flake#nixosConfigurations.<name>`) by copying system toplevel closures over SSH and invoking `switch-to-configuration`.

However, real-world infrastructure fleets are inherently heterogeneous:
1. **Agentless Network & Embedded Devices**: Routers (e.g. AVM FRITZ!Box via TR-064 SOAP/HTTPS API), Microcontrollers (ESPHome OTA), and Managed Switches have limited flash/RAM and cannot run full NixOS, yet their desired configuration state is compiled declaratively in Nix.
2. **Cloud & Edge API Targets**: DNS zones (Cloudflare, Route53), Kubernetes clusters (Helm/ArgoCD), and Cloud Ingress endpoints reconcile their state through API calls driven by Nix derivations.
3. **Non-NixOS Operating Systems**: macOS workstations running `nix-darwin` and standalone `home-manager` configurations on generic Linux distributions require orchestration without a full NixOS module system.

Forcing non-NixOS targets into `nixosConfigurations` introduces artificial coupling and heavy evaluation overhead. To serve the wider Nix and open-source community with academic rigor, `nod` must establish an entirely agnostic, first-class target orchestration protocol.

## Decision

### 1. The Universal Activation Protocol (`nodTargets`)
Introduce a first-class, top-level flake output: `nodTargets.<name>` alongside `nixosConfigurations.<name>`.

A target in `nodTargets` is either a direct derivation or an attribute set adhering to the target schema:
```nix
nodTargets.<name> = {
  # Classification & Unified Target Selection metadata (ADR-006)
  role = "router";          # desktop | notebook | server | router | embedded | cloud
  tags = [ "network" ];     # operator-assignable tags for --tag filtering
  targetHost = "10.10.10.1";# optional target IP or FQDN for reachability probing

  # Target execution modality
  targetType = "agentless"; # "agentless" (default) | "remote" | "nixos"

  # The immutable buildable artifact
  package = pkgs.writeShellApplication {
    name = "activate-${name}";
    text = ''
      # Idempotent reconciliation logic
    '';
  };
};
```

### 2. Target Modalities (`TargetKind`)
In the domain model (`HostEntity`), introduce `TargetKind`:
- `TargetKind::Nixos`: Full NixOS closure. Evaluated via `nixosConfigurations.<name>.config.system.build.toplevel`, deployed via `switch-to-configuration`.
- `TargetKind::Agentless`: Declarative reconciliation package. Evaluated via `nodTargets.<name>.package`, built on the operator machine, and executed locally against target APIs (TR-064, REST, Cloudflare, etc.).
- `TargetKind::RemoteScript`: Package closure copied to `targetHost` via SSH and executed remotely.

### 3. Hexagonal Architecture Alignment
- **Domain (`EvaluatorPort`)**: `discover_hosts` queries both `nixosConfigurations` and `nodTargets`, merging them into a unified `Vec<HostEntity>`.
- **Domain (`DeployerPort`)**: Dispatches to the appropriate deployer adapter based on `target_kind` (`SshCliDeployer`, `LocalDeployer`, or `AgentlessDeployer`).
- **Application (`DeploymentStateMachine`)**: The universal state machine (Evaluating $\to$ Building $\to$ Staging $\to$ Switching/Activating $\to$ Verifying) executes identically for all targets, ensuring full consistency in concurrency, canary rollouts, TUI dashboard metrics, and JSON audit logging.

## Consequences

### Positive
- **100% Agnostic**: Zero hardcoded strings or vendor-specific logic in `nod`.
- **Extensible & SOLID**: Open to any target type (Darwin, Home-Manager, Routers, Cloudflare, Kubernetes) without source modifications (Open/Closed Principle).
- **Zero-Friction UX**: Unified CLI grammar: `nod switch <target>`, `nod status`, `nod diff` seamlessly manage both NixOS servers and agentless infrastructure.
- **Backward Compatible**: Existing `nixosConfigurations` continue to work with zero changes.

### Negative / Trade-offs
- Discovery requires querying two flake output attributes (`nixosConfigurations` and `nodTargets`). Efficient batch evaluation mitigates overhead.
