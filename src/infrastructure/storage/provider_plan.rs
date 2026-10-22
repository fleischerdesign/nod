//! JSON storage for provider rollback plans (ADR-030).
//!
//! A plan is a record of what was applied, kept under the reconciler's state
//! directory. It is not a second declaration: if it is missing, nothing is
//! rolled back, and the desired state stays the only truth.

use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::provider::RollbackPlan;

/// Stores one plan per provider under `<state_dir>/providers/`.
pub struct ProviderPlanStore {
    root: PathBuf,
}

impl ProviderPlanStore {
    /// Builds a store rooted at `state_dir`.
    pub fn new(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join("providers"),
        }
    }

    /// The file for one provider, refusing names that could escape the root.
    fn path(&self, provider: &str) -> Result<PathBuf, NodError> {
        let valid = !provider.is_empty()
            && provider.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            });
        if !valid {
            return Err(NodError::config(format!(
                "invalid provider name `{provider}` for the plan store"
            )));
        }
        Ok(self.root.join(format!("{provider}.json")))
    }

    /// Records `plan`, replacing any previous plan for the same provider.
    pub fn save(&self, plan: &RollbackPlan) -> Result<(), NodError> {
        let path = self.path(&plan.provider)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                NodError::config(format!(
                    "cannot create plan directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let raw = serde_json::to_vec_pretty(plan)
            .map_err(|error| NodError::config(format!("cannot serialise plan: {error}")))?;
        std::fs::write(&path, raw).map_err(|error| {
            NodError::config(format!("cannot write plan {}: {error}", path.display()))
        })?;

        // The plan holds resource ids and attributes, no secrets; an operator
        // must be able to read it while the daemon writes as its account.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        }

        Ok(())
    }

    /// Reads the plan for `provider`, when one was recorded.
    pub fn load(&self, provider: &str) -> Result<Option<RollbackPlan>, NodError> {
        let path = self.path(provider)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(NodError::config(format!(
                    "cannot read plan {}: {error}",
                    path.display()
                )))
            }
        };
        let plan: RollbackPlan = serde_json::from_str(&raw).map_err(|error| {
            NodError::config(format!("invalid plan {}: {error}", path.display()))
        })?;
        Ok(Some(plan))
    }

    /// Removes the plan for `provider` after a completed rollback.
    pub fn clear(&self, provider: &str) -> Result<(), NodError> {
        let path = self.path(provider)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(NodError::config(format!(
                "cannot remove plan {}: {error}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{Change, ChangeKind, DesiredResource, InverseChange};

    fn plan(provider: &str) -> RollbackPlan {
        RollbackPlan {
            provider: provider.to_string(),
            applied_at: 1_700_000_000,
            changes: vec![InverseChange {
                change: Change {
                    kind: ChangeKind::Delete,
                    owned: true,
                    destructive: true,
                    resource: DesiredResource {
                        kind: "dns.record".to_string(),
                        id: "one".to_string(),
                        attributes: serde_json::json!({ "content": "1.2.3.4" }),
                    },
                },
                expected: None,
            }],
        }
    }

    #[test]
    fn a_plan_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProviderPlanStore::new(dir.path());
        store.save(&plan("cloudflare")).unwrap();

        let loaded = store.load("cloudflare").unwrap().expect("recorded");
        assert_eq!(loaded.provider, "cloudflare");
        assert_eq!(loaded.changes.len(), 1);

        store.clear("cloudflare").unwrap();
        assert!(store.load("cloudflare").unwrap().is_none());
    }

    #[test]
    fn a_name_that_could_escape_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProviderPlanStore::new(dir.path());
        assert!(store.load("../etc/passwd").is_err());
        assert!(store.load("").is_err());
    }
}
