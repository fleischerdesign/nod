//! Domain entities and serializers for fleet topology and inventory export (ADR-020).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Output graph visualization format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphFormat {
    Dot,
    Mermaid,
    Json,
}

impl std::str::FromStr for GraphFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "dot" => Ok(GraphFormat::Dot),
            "mermaid" => Ok(GraphFormat::Mermaid),
            "json" => Ok(GraphFormat::Json),
            other => Err(format!(
                "unknown graph format: '{other}' (expected: dot, mermaid, json)"
            )),
        }
    }
}

/// Output inventory export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Ansible,
    Prometheus,
    Json,
}

impl std::str::FromStr for ExportFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ansible" | "yaml" => Ok(ExportFormat::Ansible),
            "prometheus" | "prom" => Ok(ExportFormat::Prometheus),
            "json" => Ok(ExportFormat::Json),
            other => Err(format!(
                "unknown export format: '{other}' (expected: ansible, prometheus, json)"
            )),
        }
    }
}

/// Simplified node representation for topology graphs and exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetNode {
    pub name: String,
    pub target_host: String,
    pub tags: Vec<String>,
    pub role: Option<String>,
    pub system: Option<String>,
    pub is_local: bool,
}

/// Complete fleet topology graph and inventory collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetTopology {
    pub nodes: Vec<FleetNode>,
    pub flake_uri: String,
}

impl FleetTopology {
    /// Renders topology as a Mermaid diagram.
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("graph TD\n");
        out.push_str("  subgraph Fleet[\"NixOS Fleet\"]\n");

        for n in &self.nodes {
            let role_label = n.role.as_deref().unwrap_or("node");
            let arch_label = n.system.as_deref().unwrap_or("unknown");
            out.push_str(&format!(
                "    {}[\"{}\\n<i>role: {} | arch: {}</i>\"]\n",
                n.name, n.name, role_label, arch_label
            ));
        }

        out.push_str("  end\n");

        // Group by role subgraphs if present
        let mut roles: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for n in &self.nodes {
            if let Some(ref r) = n.role {
                roles.entry(r.as_str()).or_default().push(&n.name);
            }
        }

        for (role, members) in roles {
            out.push_str(&format!("  subgraph Role_{}[\"Role: {}\"]\n", role, role));
            for m in members {
                out.push_str(&format!("    {}\n", m));
            }
            out.push_str("  end\n");
        }

        out
    }

    /// Renders topology as Graphviz DOT.
    pub fn to_dot(&self) -> String {
        let mut out = String::from("digraph FleetTopology {\n");
        out.push_str("  rankdir=LR;\n");
        out.push_str("  node [shape=box, style=\"rounded,filled\", fillcolor=\"#f0f4f8\", fontname=\"Helvetica\"];\n");

        for n in &self.nodes {
            let label = format!(
                "{}\\n({})\\nrole: {}",
                n.name,
                n.target_host,
                n.role.as_deref().unwrap_or("-")
            );
            out.push_str(&format!("  \"{}\" [label=\"{}\"];\n", n.name, label));
        }

        out.push_str("}\n");
        out
    }

    /// Renders topology as an Ansible YAML inventory.
    pub fn to_ansible_inventory(&self) -> String {
        let mut all_hosts = BTreeMap::new();
        let mut by_role: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut by_tag: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        for n in &self.nodes {
            let mut host_vars = BTreeMap::new();
            host_vars.insert("ansible_host", n.target_host.clone());
            if let Some(ref s) = n.system {
                host_vars.insert("nix_system", s.clone());
            }
            all_hosts.insert(n.name.clone(), host_vars);

            if let Some(ref r) = n.role {
                by_role.entry(r.clone()).or_default().insert(n.name.clone());
            }
            for t in &n.tags {
                by_tag.entry(t.clone()).or_default().insert(n.name.clone());
            }
        }

        let mut out = String::from("---\nall:\n  hosts:\n");
        for (name, vars) in all_hosts {
            out.push_str(&format!("    {}:\n", name));
            for (k, v) in vars {
                out.push_str(&format!("      {}: \"{}\"\n", k, v));
            }
        }

        if !by_role.is_empty() || !by_tag.is_empty() {
            out.push_str("  children:\n");
            for (role, hosts) in by_role {
                out.push_str(&format!("    role_{}:\n      hosts:\n", role));
                for h in hosts {
                    out.push_str(&format!("        {}:\n", h));
                }
            }
            for (tag, hosts) in by_tag {
                out.push_str(&format!("    tag_{}:\n      hosts:\n", tag));
                for h in hosts {
                    out.push_str(&format!("        {}:\n", h));
                }
            }
        }

        out
    }

    /// Renders topology as Prometheus HTTP/File Service Discovery JSON.
    pub fn to_prometheus_sd(&self) -> String {
        #[derive(Serialize)]
        struct PromTargetGroup {
            targets: Vec<String>,
            labels: BTreeMap<String, String>,
        }

        let groups: Vec<PromTargetGroup> = self
            .nodes
            .iter()
            .map(|n| {
                let mut labels = BTreeMap::new();
                labels.insert("instance".to_string(), n.name.clone());
                if let Some(ref r) = n.role {
                    labels.insert("role".to_string(), r.clone());
                }
                if let Some(ref s) = n.system {
                    labels.insert("arch".to_string(), s.clone());
                }
                if !n.tags.is_empty() {
                    labels.insert("tags".to_string(), n.tags.join(","));
                }

                // Default node_exporter port 9100
                let target_addr = if n.target_host.contains(':') {
                    n.target_host.clone()
                } else {
                    format!("{}:9100", n.target_host)
                };

                PromTargetGroup {
                    targets: vec![target_addr],
                    labels,
                }
            })
            .collect();

        serde_json::to_string_pretty(&groups).unwrap_or_else(|_| "[]".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_renders_valid_mermaid_and_dot() {
        let topo = FleetTopology {
            nodes: vec![
                FleetNode {
                    name: "yorke".to_string(),
                    target_host: "10.0.0.1".to_string(),
                    tags: vec!["core".to_string()],
                    role: Some("server".to_string()),
                    system: Some("x86_64-linux".to_string()),
                    is_local: false,
                },
                FleetNode {
                    name: "selway".to_string(),
                    target_host: "10.0.0.2".to_string(),
                    tags: vec!["edge".to_string()],
                    role: Some("gateway".to_string()),
                    system: Some("aarch64-linux".to_string()),
                    is_local: false,
                },
            ],
            flake_uri: ".".to_string(),
        };

        let mermaid = topo.to_mermaid();
        assert!(mermaid.contains("yorke"));
        assert!(mermaid.contains("selway"));
        assert!(mermaid.contains("Role: server"));

        let dot = topo.to_dot();
        assert!(dot.contains("digraph FleetTopology"));
        assert!(dot.contains("\"yorke\""));

        let ansible = topo.to_ansible_inventory();
        assert!(ansible.contains("role_server:"));
        assert!(ansible.contains("ansible_host: \"10.0.0.1\""));

        let prom = topo.to_prometheus_sd();
        assert!(prom.contains("10.0.0.1:9100"));
        assert!(prom.contains("\"instance\": \"yorke\""));
    }
}
