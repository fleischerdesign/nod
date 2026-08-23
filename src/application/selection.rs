//! Member selection helpers shared by the CLI/TUI use cases.
//!
//! Selection is a pure function over the discovered host list (ADR-006):
//! every supplied criterion — target/glob, `--all`, `--tag`, `--role` — is
//! combined by boolean AND, so the selector is order free and never touches
//! Nix or SSH.

use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Pure host selection/filtering logic (target `all`, `local`, a named host
/// or a glob, narrowed by optional tag/role filters). Extracted so it can be
/// unit tested without Nix or SSH.
pub struct TargetSelection;

impl TargetSelection {
    /// Shell-style wildcard match supporting `*` (any run of characters) and
    /// `?` (exactly one character).
    fn glob_match(pattern: &str, text: &str) -> bool {
        let pattern: Vec<char> = pattern.chars().collect();
        let text: Vec<char> = text.chars().collect();
        let (mut p, mut t) = (0, 0);
        let mut star_p: Option<usize> = None;
        let mut star_t = 0;
        while t < text.len() {
            if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
                p += 1;
                t += 1;
            } else if p < pattern.len() && pattern[p] == '*' {
                star_p = Some(p);
                star_t = t;
                p += 1;
            } else if let Some(sp) = star_p {
                star_t += 1;
                t = star_t;
                p = sp + 1;
            } else {
                return false;
            }
        }
        while p < pattern.len() && pattern[p] == '*' {
            p += 1;
        }
        p == pattern.len()
    }

    /// Returns `true` when the target contains glob metacharacters and must
    /// be matched as a pattern rather than an exact host name.
    fn is_glob(target: &str) -> bool {
        target.contains('*') || target.contains('?')
    }

    /// Applies the name-level selector (`target` / `--all`) to one host.
    /// Tag/role filters are applied separately so every criterion composes
    /// by boolean AND (ADR-006).
    fn name_matches(
        host: &HostEntity,
        target: Option<&str>,
        all: bool,
        local_hostname: &str,
    ) -> bool {
        if all {
            return true;
        }
        match target {
            Some("all") => true,
            Some("local") => host.is_local || host.name == local_hostname,
            Some(t) if Self::is_glob(t) => Self::glob_match(t, &host.name),
            Some(t) => host.name == t,
            // Degenerate call: no target and no `--all` is the empty
            // (identity) selector — the whole candidate set, still
            // narrowable by tag/role.
            None => true,
        }
    }

    /// Describes every active criterion for error messages in a stable
    /// order (target/`--all`, then tag, then role).
    fn criteria_parts(
        target: Option<&str>,
        tag: Option<&str>,
        role: Option<&str>,
        all: bool,
    ) -> Vec<String> {
        let mut parts: Vec<String> = Vec::new();
        match target {
            Some("all") => parts.push("target 'all'".to_string()),
            Some("local") => parts.push("target 'local'".to_string()),
            Some(t) => parts.push(format!("target '{t}'")),
            None => {
                if all {
                    parts.push("--all".to_string());
                }
            }
        }
        if let Some(tag) = tag {
            parts.push(format!("tag '{tag}'"));
        }
        if let Some(role) = role {
            parts.push(format!("role '{role}'"));
        }
        parts
    }

    /// Filters discovered hosts according to the target specifier, optional
    /// `--tag` / `--role` filters and the `--all` flag.
    ///
    /// Semantics (ADR-006):
    /// - `all == true` (or the target sentinel `all`) makes every discovered
    ///   host a candidate; tag/role filters still narrow the set via AND.
    /// - target `local` selects the host whose `is_local` flag is set or
    ///   whose name equals `local_hostname`.
    /// - a target containing `*` / `?` glob-matches host names.
    /// - any other target is an exact host-name match.
    pub fn select(
        hosts: Vec<HostEntity>,
        target: Option<&str>,
        tag: Option<&str>,
        role: Option<&str>,
        all: bool,
        local_hostname: &str,
    ) -> Vec<HostEntity> {
        hosts
            .into_iter()
            .filter(|h| Self::name_matches(h, target, all, local_hostname))
            .filter(|h| match tag {
                Some(t) => h.has_tag(t),
                None => true,
            })
            .filter(|h| match role {
                Some(r) => h.matches_role(r),
                None => true,
            })
            .collect()
    }

    /// Selects exactly one host for single-terminal commands (`ssh`, `repl`,
    /// ADR-006). Fails with a `no hosts matched` error naming the criteria
    /// when nothing matches, and with a `multiple hosts matched` error
    /// listing the candidates when more than one host matches; otherwise it
    /// returns the single winning host.
    pub fn select_exact_one(
        hosts: Vec<HostEntity>,
        target: Option<&str>,
        tag: Option<&str>,
        role: Option<&str>,
        all: bool,
        local_hostname: &str,
    ) -> Result<HostEntity, NodError> {
        let matched = Self::select(hosts, target, tag, role, all, local_hostname);
        match matched.len() {
            0 => {
                let criteria = Self::criteria_parts(target, tag, role, all).join(", ");
                let msg = if criteria.is_empty() {
                    "no hosts matched".to_string()
                } else {
                    format!("no hosts matched {criteria}")
                };
                Err(NodError::config(msg))
            }
            1 => {
                let mut matched = matched.into_iter();
                Ok(matched.next().expect("exactly one host matched"))
            }
            _ => {
                let names: Vec<String> = matched.iter().map(|h| h.name.clone()).collect();
                let criteria = Self::criteria_parts(target, tag, role, all).join(", ");
                let msg = if criteria.is_empty() {
                    format!(
                        "multiple hosts matched [{}]; use a single host name or narrow the filters",
                        names.join(", ")
                    )
                } else {
                    format!(
                        "multiple hosts matched {criteria} [{}]; use a single host name or narrow the filters",
                        names.join(", ")
                    )
                };
                Err(NodError::config(msg))
            }
        }
    }

    /// Filters hosts selected by the positional `target` and narrows by
    /// optional `tag` / `role` filters (empty filters match everything).
    ///
    /// Compatibility shim over [`TargetSelection::select`] kept for the
    /// deployed commands; the unified API prefers the `Option<&str>` target
    /// and separate `all` flag.
    pub fn select_filtered(
        hosts: Vec<HostEntity>,
        target: &str,
        local_hostname: &str,
        tag: Option<&str>,
        role: Option<&str>,
    ) -> Vec<HostEntity> {
        Self::select(
            hosts,
            Some(target),
            tag,
            role,
            target == "all",
            local_hostname,
        )
    }

    /// Builds the "no host matched" error for the target, naming any active
    /// tag/role filters.
    pub fn unmatched(target: &str, tag: Option<&str>, role: Option<&str>) -> NodError {
        let criteria = Self::criteria_parts(Some(target), tag, role, false).join(", ");
        let msg = if criteria.is_empty() {
            format!("no hosts matched target '{target}'")
        } else {
            format!("no hosts matched {criteria}")
        };
        NodError::config(msg)
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

    /// The ADR-006 spec fixture fleet (`unified-target-selection.spec.md`).
    fn spec_fleet() -> Vec<HostEntity> {
        let host = |name: &str, tags: &[&str], role: HostRole| {
            let mut h = HostEntity::new(name, name, false);
            h.tags = tags.iter().map(|t| t.to_string()).collect();
            h.role = role;
            h
        };
        vec![
            host("web-01", &["prod", "web"], HostRole::Server),
            host("web-02", &["prod", "web"], HostRole::Server),
            host("web-staging", &["staging", "web"], HostRole::Server),
            host("db-01", &["prod", "db"], HostRole::Server),
            host("db-prod-01", &["prod", "db"], HostRole::Server),
            host("edge-prod", &["prod", "edge"], HostRole::Notebook),
        ]
    }

    #[test]
    fn select_all_keeps_every_host() {
        let hosts = TargetSelection::select(fleet(), Some("all"), None, None, false, "jello");
        assert_eq!(hosts.len(), 3);
    }

    #[test]
    fn select_local_matches_by_hostname_or_locality() {
        let hosts = TargetSelection::select(fleet(), Some("local"), None, None, false, "jello");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "jello");

        let hosts2 =
            TargetSelection::select(fleet(), Some("local"), None, None, false, "unknown-machine");
        assert_eq!(hosts2.len(), 1);
        assert!(hosts2[0].is_local);
    }

    #[test]
    fn select_named_host_returns_only_matches() {
        let hosts = TargetSelection::select(fleet(), Some("atlas"), None, None, false, "jello");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "atlas");
    }

    #[test]
    fn select_unknown_target_returns_empty() {
        let hosts = TargetSelection::select(fleet(), Some("nowhere"), None, None, false, "jello");
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
        let hosts =
            TargetSelection::select_filtered(fleet(), "all", "jello", None, Some("desktop"));
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "jello");
    }

    #[test]
    fn select_filtered_combines_tag_and_role() {
        let hosts = TargetSelection::select_filtered(
            fleet(),
            "all",
            "jello",
            Some("server"),
            Some("server"),
        );
        assert_eq!(hosts.len(), 2);

        let no_match = TargetSelection::select_filtered(
            fleet(),
            "all",
            "jello",
            Some("prod"),
            Some("desktop"),
        );
        assert!(no_match.is_empty());
    }

    #[test]
    fn select_filtered_applies_to_a_named_target() {
        let hosts = TargetSelection::select_filtered(fleet(), "atlas", "jello", Some("prod"), None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "atlas");

        let none =
            TargetSelection::select_filtered(fleet(), "atlas", "jello", Some("laptop"), None);
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

    // --- ADR-006: exact name match ---

    #[test]
    fn select_exact_target_resolves_that_host_only() {
        let hosts = TargetSelection::select(spec_fleet(), Some("web-01"), None, None, false, "n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "web-01");
    }

    #[test]
    fn select_exact_target_match_nothing_returns_empty() {
        let hosts =
            TargetSelection::select(spec_fleet(), Some("nowhere-01"), None, None, false, "n");
        assert!(hosts.is_empty());
    }

    // --- ADR-006: glob pattern matching ---

    #[test]
    fn select_glob_prefix_matches_by_hostname() {
        let hosts = TargetSelection::select(spec_fleet(), Some("web-*"), None, None, false, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["web-01", "web-02", "web-staging"]);
    }

    #[test]
    fn select_glob_suffix_matches_by_hostname() {
        // Standard shell-glob semantics: `*-prod` anchors on the end of the
        // name, so only host names ending in `-prod` match. The spec fixture
        // lists `[db-prod-01, edge-prod]`, but `db-prod-01` ends in `-01` and
        // therefore is *not* matched by a trailing `-prod` anchor.
        let hosts = TargetSelection::select(spec_fleet(), Some("*-prod"), None, None, false, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["edge-prod"]);
    }

    #[test]
    fn select_glob_infix_matches_by_hostname() {
        let hosts = TargetSelection::select(spec_fleet(), Some("*db*"), None, None, false, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["db-01", "db-prod-01"]);
    }

    #[test]
    fn select_glob_single_char_wildcard_matches_by_hostname() {
        let hosts = TargetSelection::select(spec_fleet(), Some("web-0?"), None, None, false, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["web-01", "web-02"]);
    }

    #[test]
    fn select_glob_with_no_matches_returns_empty() {
        let hosts =
            TargetSelection::select(spec_fleet(), Some("nonexistent-*"), None, None, false, "n");
        assert!(hosts.is_empty());
    }

    // --- ADR-006: tag filter ---

    #[test]
    fn select_tag_filter_narrows_the_fleet() {
        let hosts = TargetSelection::select(spec_fleet(), None, Some("prod"), None, true, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(
            names,
            ["web-01", "web-02", "db-01", "db-prod-01", "edge-prod"]
        );
    }

    #[test]
    fn select_tag_with_no_holder_matches_nothing() {
        let hosts = TargetSelection::select(spec_fleet(), None, Some("database"), None, true, "n");
        assert!(hosts.is_empty());
    }

    // --- ADR-006: role filter ---

    #[test]
    fn select_role_filter_narrows_the_fleet() {
        let hosts = TargetSelection::select(spec_fleet(), None, None, Some("server"), true, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(
            names,
            ["web-01", "web-02", "web-staging", "db-01", "db-prod-01"]
        );
    }

    #[test]
    fn select_role_filter_combines_with_a_glob() {
        let hosts =
            TargetSelection::select(spec_fleet(), Some("db-*"), None, Some("server"), false, "n");
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["db-01", "db-prod-01"]);
    }

    // --- ADR-006: boolean AND intersection ---

    #[test]
    fn select_intersects_glob_tag_and_role() {
        let hosts = TargetSelection::select(
            spec_fleet(),
            Some("web-*"),
            Some("prod"),
            Some("server"),
            false,
            "n",
        );
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["web-01", "web-02"]);
    }

    #[test]
    fn select_contradictory_intersection_matches_nothing() {
        let hosts =
            TargetSelection::select(spec_fleet(), Some("web-*"), Some("db"), None, false, "n");
        assert!(hosts.is_empty());
    }

    // --- ADR-006: universal --all flag ---

    #[test]
    fn select_all_flag_selects_every_host() {
        let hosts = TargetSelection::select(spec_fleet(), None, None, None, true, "n");
        assert_eq!(hosts.len(), 6);
    }

    #[test]
    fn select_all_flag_composes_with_filters() {
        let hosts = TargetSelection::select(spec_fleet(), None, None, Some("notebook"), true, "n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "edge-prod");
    }

    #[test]
    fn select_all_sentinel_equals_the_all_flag() {
        let via_flag = TargetSelection::select(spec_fleet(), None, None, None, true, "n");
        let via_target = TargetSelection::select(spec_fleet(), Some("all"), None, None, false, "n");
        assert_eq!(via_flag.len(), via_target.len());
        assert_eq!(via_flag[0].name, via_target[0].name);
    }

    // --- ADR-006: single-target restriction (select_exact_one) ---

    #[test]
    fn select_exact_one_resolves_a_single_exact_host() {
        let host =
            TargetSelection::select_exact_one(spec_fleet(), Some("web-01"), None, None, false, "n")
                .expect("exact name matches exactly one host");
        assert_eq!(host.name, "web-01");
    }

    #[test]
    fn select_exact_one_resolves_a_single_narrowed_host() {
        let host =
            TargetSelection::select_exact_one(spec_fleet(), None, Some("edge"), None, true, "n")
                .expect("tag filter matches exactly one host");
        assert_eq!(host.name, "edge-prod");
    }

    #[test]
    fn select_exact_one_rejects_zero_matches() {
        let err = TargetSelection::select_exact_one(
            spec_fleet(),
            Some("nonexistent-*"),
            None,
            None,
            false,
            "n",
        )
        .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        let msg = err.to_string();
        assert!(msg.contains("no hosts matched"));
        assert!(msg.contains("nonexistent-*"));
    }

    #[test]
    fn select_exact_one_zero_match_error_lists_the_criteria() {
        let err = TargetSelection::select_exact_one(
            spec_fleet(),
            Some("web-*"),
            Some("prod"),
            Some("notebook"),
            false,
            "n",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no hosts matched"));
        assert!(msg.contains("web-*"));
        assert!(msg.contains("tag 'prod'"));
        assert!(msg.contains("role 'notebook'"));
    }

    #[test]
    fn select_exact_one_rejects_multiple_matches_and_lists_them() {
        let err =
            TargetSelection::select_exact_one(spec_fleet(), Some("web-*"), None, None, false, "n")
                .unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        let msg = err.to_string();
        assert!(msg.contains("multiple hosts matched"));
        assert!(msg.contains("web-01"));
        assert!(msg.contains("web-02"));
        assert!(msg.contains("web-staging"));
    }
}
