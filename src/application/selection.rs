//! Member selection helpers shared by the CLI/TUI use cases.

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Pure host selection/filtering logic (target `all`, `local` or a named
/// host, narrowed by optional tag/role filters). Extracted so it can be unit
/// tested without Nix or SSH.
pub struct TargetSelection;

impl TargetSelection {
    /// Filters discovered hosts according to the target specifier.
    pub fn select(
        hosts: Vec<HostEntity>,
        target: &str,
        local_hostname: &str,
    ) -> Vec<HostEntity> {
        if target == "all" {
            hosts
        } else if target == "local" {
            hosts
                .into_iter()
                .filter(|h| h.name == local_hostname || h.is_local)
                .collect()
        } else {
            hosts
                .into_iter()
                .filter(|h| h.name == target)
                .collect()
        }
    }

    /// Selects hosts by target and then narrows by optional `tag` / `role`
    /// filters (empty filters match everything).
    pub fn select_filtered(
        hosts: Vec<HostEntity>,
        target: &str,
        local_hostname: &str,
        tag: Option<&str>,
        role: Option<&str>,
    ) -> Vec<HostEntity> {
        Self::select(hosts, target, local_hostname)
            .into_iter()
            .filter(|h| {
                match tag {
                    Some(t) => h.has_tag(t),
                    None => true,
                }
            })
            .filter(|h| {
                match role {
                    Some(r) => h.matches_role(r),
                    None => true,
                }
            })
            .collect()
    }

    /// Builds the "no host matched" error for the target, naming any active
    /// tag/role filters.
    pub fn unmatched(target: &str, tag: Option<&str>, role: Option<&str>) -> NodError {
        let details = match (tag, role) {
            (Some(t), Some(r)) => format!(", tag '{t}', role '{r}'"),
            (Some(t), None) => format!(", tag '{t}'"),
            (None, Some(r)) => format!(", role '{r}'"),
            (None, None) => String::new(),
        };
        NodError::config(format!("no hosts matched target '{target}'{details}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostRole;

    fn fleet() -> Vec<HostEntity> {
        let mut jello = HostEntity::new("jello", "jello-machine", true);
        jello.tags = vec!["laptop".to_string(), "home".to_string()];
        jello.role = HostRole::Desktop;

        let mut atlas = HostEntity::new("atlas", "10.0.0.8", false);
        atlas.tags = vec!["server".to_string(), "prod".to_string()];

        let mut orbit = HostEntity::new("orbit", "10.0.0.9", false);
        orbit.tags = vec!["server".to_string()];

        vec![jello, atlas, orbit]
    }

    #[test]
    fn select_all_keeps_every_host() {
        let hosts = TargetSelection::select(fleet(), "all", "jello");
        assert_eq!(hosts.len(), 3);
    }

    #[test]
    fn select_local_matches_by_hostname_or_locality() {
        let hosts = TargetSelection::select(fleet(), "local", "jello");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "jello");

        let hosts2 = TargetSelection::select(fleet(), "local", "unknown-machine");
        assert_eq!(hosts2.len(), 1);
        assert!(hosts2[0].is_local);
    }

    #[test]
    fn select_named_host_returns_only_matches() {
        let hosts = TargetSelection::select(fleet(), "atlas", "jello");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "atlas");
    }

    #[test]
    fn select_unknown_target_returns_empty() {
        let hosts = TargetSelection::select(fleet(), "nowhere", "jello");
        assert!(hosts.is_empty());
    }

    #[test]
    fn select_filtered_by_tag_narrows_the_target() {
        let hosts = TargetSelection::select_filtered(fleet(), "all", "jello", Some("server"), None);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "atlas");
        assert_eq!(hosts[1].name, "orbit");
    }

    #[test]
    fn select_filtered_by_role_narrows_the_target() {
        let hosts = TargetSelection::select_filtered(fleet(), "all", "jello", None, Some("desktop"));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "jello");
    }

    #[test]
    fn select_filtered_combines_tag_and_role() {
        let hosts =
            TargetSelection::select_filtered(fleet(), "all", "jello", Some("server"), Some("server"));
        assert_eq!(hosts.len(), 2);

        let no_match =
            TargetSelection::select_filtered(fleet(), "all", "jello", Some("prod"), Some("desktop"));
        assert!(no_match.is_empty());
    }

    #[test]
    fn select_filtered_applies_to_a_named_target() {
        let hosts = TargetSelection::select_filtered(fleet(), "atlas", "jello", Some("prod"), None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "atlas");

        let none = TargetSelection::select_filtered(fleet(), "atlas", "jello", Some("laptop"), None);
        assert!(none.is_empty());
    }

    #[test]
    fn select_filtered_without_filters_preserves_target_behavior() {
        let hosts = TargetSelection::select_filtered(fleet(), "all", "jello", None, None);
        assert_eq!(hosts.len(), 3);

        let local = TargetSelection::select_filtered(fleet(), "local", "jello", None, None);
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].name, "jello");
    }
}