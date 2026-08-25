# ADR-020: Fleet Topology Graph & Inventory Exporters (`nod graph`, `nod export`)

## Context
Operators managing multi-host NixOS environments need integrations with external observability and automation platforms, as well as visual inspection of fleet topology:
1. **Visual Dependency & Topology Inspection** (`nod graph`): Rendering fleet node relations, hardware architectures, tags, and roles in standard visualization formats (`mermaid`, `dot` / Graphviz, and structured JSON).
2. **Third-Party Infrastructure Integration** (`nod export`): Exporting the evaluated fleet to standard inventory formats:
   - **Ansible Inventory**: YAML/INI inventory format grouping hosts by `role` and `tag`.
   - **Prometheus Service Discovery**: `http_sd` / `file_sd` JSON format for Prometheus target scraping (e.g. node_exporter endpoints).
   - **Generic JSON**: Universal schema for downstream CI/CD pipelines.

## Decision
1. **Domain Layer (`src/domain/topology.rs`)**:
   - Pure, dependency-free domain models `FleetNode` and `FleetTopology`.
   - Pure serialization methods: `to_mermaid()`, `to_dot()`, `to_ansible_inventory()`, and `to_prometheus_sd()`.
2. **Application Layer**:
   - `RenderGraphUseCase` (`src/application/use_cases/render_graph.rs`).
   - `ExportInventoryUseCase` (`src/application/use_cases/export_inventory.rs`).
3. **Presentation Layer**:
   - `nod graph [--format dot|mermaid|json] [--flake <PATH>]`
   - `nod export <FORMAT> [--flake <PATH>]` (formats: `ansible`, `prometheus`, `json`).

## Consequences
- **Positive**: Direct zero-dependency interoperability with Ansible, Prometheus, and Grafana.
- **Positive**: Clean visual topology rendering natively supported in Markdown/Mermaid viewers and Graphviz.
- **Positive**: 100% test coverage and pure domain separation.
