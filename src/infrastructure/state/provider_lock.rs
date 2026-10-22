//! Per-provider lock (ADR-030).
//!
//! A one-shot `nod reconcile` and the daemon must not reconcile the same
//! provider at the same time: two writers against one namespace is how a
//! reconciler fights itself. The lock is a file with the owning pid; a lock
//! whose owner is gone, or which is older than an hour, is reclaimed.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::domain::errors::NodError;

/// A lock older than this is abandoned even when its pid is still alive.
const STALE_AFTER: Duration = Duration::from_secs(3600);

/// Held for the duration of one provider reconciliation.
pub struct ProviderLock {
    path: PathBuf,
}

impl ProviderLock {
    /// Acquires the lock for `provider` under `<state_dir>/providers/`.
    pub fn acquire(state_dir: &Path, provider: &str) -> Result<Self, NodError> {
        let valid = !provider.is_empty()
            && provider.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            });
        if !valid {
            return Err(NodError::config(format!(
                "invalid provider name `{provider}` for the lock"
            )));
        }

        let directory = state_dir.join("providers");
        fs::create_dir_all(&directory).map_err(|error| {
            NodError::internal(format!(
                "cannot create lock directory {}: {error}",
                directory.display()
            ))
        })?;
        // A 0700 lock directory would also hide the rollback plans stored next
        // to it; operators must be able to read them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&directory, fs::Permissions::from_mode(0o755));
        }
        let path = directory.join(format!("{provider}.lock"));

        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && is_stale(&path) {
                        fs::remove_file(&path).map_err(|error| {
                            NodError::internal(format!(
                                "cannot reclaim stale lock {}: {error}",
                                path.display()
                            ))
                        })?;
                        continue;
                    }
                    return Err(NodError::config(format!(
                        "provider `{provider}` is already being reconciled (lock {})",
                        path.display()
                    )));
                }
                Err(error) => {
                    return Err(NodError::internal(format!(
                        "cannot create lock {}: {error}",
                        path.display()
                    )))
                }
            }
        }

        Err(NodError::internal("lock acquisition loop exhausted"))
    }
}

impl Drop for ProviderLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a lock file belongs to a dead process or outlived its welcome.
fn is_stale(path: &Path) -> bool {
    if let Ok(raw) = fs::read_to_string(path) {
        if let Ok(pid) = raw.trim().parse::<u32>() {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
        }
    }

    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > STALE_AFTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_is_refused_while_the_first_is_alive() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProviderLock::acquire(dir.path(), "cloudflare").unwrap();
        let second = ProviderLock::acquire(dir.path(), "cloudflare");
        assert!(second.is_err(), "a live lock must not be taken twice");
        drop(first);

        let again = ProviderLock::acquire(dir.path(), "cloudflare");
        assert!(again.is_ok(), "the lock is released on drop");
    }

    #[test]
    fn a_dead_owner_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers");
        fs::create_dir_all(&path).unwrap();
        // A pid that cannot exist: the kernel's pid_max is far below this.
        fs::write(path.join("cloudflare.lock"), "999999999\n").unwrap();

        assert!(ProviderLock::acquire(dir.path(), "cloudflare").is_ok());
    }

    #[test]
    fn names_that_could_escape_the_directory_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ProviderLock::acquire(dir.path(), "../etc").is_err());
        assert!(ProviderLock::acquire(dir.path(), "").is_err());
    }
}
