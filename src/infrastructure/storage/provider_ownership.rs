//! JSON storage for the ownership record (ADR-037).
//!
//! The record lives next to the rollback plan under the reconciler's state
//! directory. It is not a second declaration: if it is missing or unreadable,
//! marker-less resources become foreign and are reported as unowned, which is
//! the safe direction.

use std::path::{Path, PathBuf};

use crate::domain::errors::NodError;
use crate::domain::ownership::Ownership;

/// Stores one record per provider under `<state_dir>/providers/`.
pub struct ProviderOwnershipStore {
    root: PathBuf,
}

impl ProviderOwnershipStore {
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
                "invalid provider name `{provider}` for the ownership store"
            )));
        }
        Ok(self.root.join(format!("{provider}.ownership.json")))
    }

    /// Records `ownership`, replacing any previous record for the provider.
    pub fn save(&self, ownership: &Ownership) -> Result<(), NodError> {
        let path = self.path(&ownership.provider)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                NodError::config(format!(
                    "cannot create ownership directory {}: {error}",
                    parent.display()
                ))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755));
            }
        }

        let raw = serde_json::to_vec_pretty(ownership)
            .map_err(|error| NodError::config(format!("cannot serialise ownership: {error}")))?;
        std::fs::write(&path, raw).map_err(|error| {
            NodError::config(format!(
                "cannot write ownership {}: {error}",
                path.display()
            ))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        }

        Ok(())
    }

    /// Reads the record for `provider`, when one was written.
    pub fn load(&self, provider: &str) -> Result<Option<Ownership>, NodError> {
        let path = self.path(provider)?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(NodError::config(format!(
                    "cannot read ownership {}: {error}",
                    path.display()
                )))
            }
        };
        let ownership: Ownership = serde_json::from_str(&raw).map_err(|error| {
            NodError::config(format!("invalid ownership {}: {error}", path.display()))
        })?;
        Ok(Some(ownership))
    }

    /// Removes the record for `provider`.
    pub fn clear(&self, provider: &str) -> Result<(), NodError> {
        let path = self.path(provider)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(NodError::config(format!(
                "cannot remove ownership {}: {error}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ownership::OwnedId;

    fn record(provider: &str) -> Ownership {
        let mut record = Ownership {
            provider: provider.to_string(),
            ..Ownership::default()
        };
        record.rules.insert(
            "per-member".to_string(),
            vec![OwnedId::new("authentik.group", "ada")],
        );
        record.learn("authentik.application", "static");
        record
    }

    #[test]
    fn a_record_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProviderOwnershipStore::new(dir.path());
        store.save(&record("authentik")).unwrap();

        let loaded = store.load("authentik").unwrap().expect("recorded");
        assert_eq!(loaded.provider, "authentik");
        assert!(loaded.contains("per-member", "authentik.group", "ada"));
        assert!(!loaded.contains("per-member", "authentik.group", "zoe"));
        assert!(loaded.owns("authentik.application", "static"));

        store.clear("authentik").unwrap();
        assert!(store.load("authentik").unwrap().is_none());
    }

    #[test]
    fn a_name_that_could_escape_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProviderOwnershipStore::new(dir.path());
        assert!(store.load("../etc/passwd").is_err());
        assert!(store.load("").is_err());
    }
}
