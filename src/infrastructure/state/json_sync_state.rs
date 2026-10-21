//! JSON state adapter and single-reconciler lock (ADR-028).
//!
//! State is written atomically (temp file + rename) so a crash never leaves a
//! truncated document behind. The lock is a PID file: a second reconciler is
//! refused, and a lock left by a dead process is reclaimed.

use async_trait::async_trait;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::domain::errors::NodError;
use crate::domain::ports::sync_state::SyncStatePort;
use crate::domain::sync::{SyncState, STATE_VERSION};

/// Name of the state document inside the state directory.
const STATE_FILE: &str = "state.json";

/// Name of the lock file inside the state directory.
const LOCK_FILE: &str = "sync.lock";

/// A lock older than this is considered abandoned when the owning process
/// cannot be probed (non-Linux hosts, missing `/proc`).
const LOCK_STALE_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// Stores [`SyncState`] as a versioned JSON document.
#[derive(Debug, Clone)]
pub struct JsonSyncStateStore {
    path: PathBuf,
}

impl JsonSyncStateStore {
    /// Creates the store inside `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(STATE_FILE),
        }
    }

    /// Creates the store at an explicit document path.
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// The document path, for operator-facing messages.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl SyncStatePort for JsonSyncStateStore {
    async fn load(&self) -> Result<Option<SyncState>, NodError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(NodError::internal(format!(
                    "cannot read sync state {}: {e}",
                    self.path.display()
                )))
            }
        };

        let state = serde_json::from_slice::<SyncState>(&bytes).map_err(|e| {
            NodError::internal(format!("invalid sync state {}: {e}", self.path.display()))
        })?;
        // A document written by a schema this build does not know is not
        // something to guess at. Older documents are read with defaults; newer
        // ones are treated as no memory at all, which costs one sweep.
        if state.version > STATE_VERSION {
            return Ok(None);
        }
        Ok(Some(state))
    }

    async fn save(&self, state: &SyncState) -> Result<(), NodError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                NodError::internal(format!(
                    "cannot create state directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        // The version describes the schema that wrote the document. An older
        // document is read with defaults, so the first save stamps the current
        // version rather than preserving a number that no longer describes it.
        let mut state = state.clone();
        state.version = STATE_VERSION;
        // The age of this document is the age of everything the status
        // command reports, so it is stamped rather than inferred from mtime.
        state.saved_at = Some(crate::domain::sync::now_secs());
        let serialized = serde_json::to_vec_pretty(&state)
            .map_err(|e| NodError::internal(format!("cannot serialize sync state: {e}")))?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, &serialized).map_err(|e| {
            NodError::internal(format!(
                "cannot write sync state {}: {e}",
                temporary.display()
            ))
        })?;
        // The document holds revisions, host names and counters - no secrets -
        // and `nod sync status` must be able to read it as a normal user. The
        // daemon's own umask is deliberately strict, so the mode is set here
        // instead of widening it for every file the unit touches.
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).map_err(|e| {
            NodError::internal(format!(
                "cannot make sync state readable {}: {e}",
                temporary.display()
            ))
        })?;
        fs::rename(&temporary, &self.path).map_err(|e| {
            NodError::internal(format!(
                "cannot replace sync state {}: {e}",
                self.path.display()
            ))
        })
    }
}

/// Exclusive ownership of the reconciler for one state directory.
///
/// Dropping the guard removes the lock file. A guard is intentionally not
/// cloneable; ownership is the mutual exclusion.
#[derive(Debug)]
pub struct SyncLock {
    path: PathBuf,
}

impl SyncLock {
    /// `true` when a live reconciler currently holds the lock for `dir`.
    ///
    /// Read-only commands use this to say that the checkout may move
    /// underneath them instead of waiting for the writer.
    pub fn held(dir: &Path) -> bool {
        let path = dir.join(LOCK_FILE);
        path.exists() && !lock_is_stale(&path)
    }

    /// Acquires the lock, reclaiming it when the recorded owner is dead.
    pub fn acquire(dir: &Path) -> Result<Self, NodError> {
        fs::create_dir_all(dir).map_err(|e| {
            NodError::internal(format!(
                "cannot create state directory {}: {e}",
                dir.display()
            ))
        })?;

        let path = dir.join(LOCK_FILE);
        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && lock_is_stale(&path) {
                        fs::remove_file(&path).map_err(|e| {
                            NodError::internal(format!(
                                "cannot reclaim stale lock {}: {e}",
                                path.display()
                            ))
                        })?;
                        continue;
                    }
                    return Err(NodError::config(format!(
                        "another reconciliation holds the lock {}; \
                         remove it only when no nod process is running",
                        path.display()
                    )));
                }
                Err(e) => {
                    return Err(NodError::internal(format!(
                        "cannot create lock {}: {e}",
                        path.display()
                    )))
                }
            }
        }

        Err(NodError::internal("lock acquisition loop exhausted"))
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// `true` when the recorded owner is provably gone.
fn lock_is_stale(path: &Path) -> bool {
    let pid = match fs::read_to_string(path) {
        Ok(text) => text.trim().parse::<u32>().ok(),
        Err(_) => None,
    };

    // Linux: a live PID owns a /proc entry. A dead one does not.
    if Path::new("/proc").is_dir() {
        if let Some(pid) = pid {
            return !Path::new(&format!("/proc/{pid}")).exists();
        }
    }

    // Without a probe, fall back to age so an abandoned lock cannot block a
    // host forever.
    match fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .map(|age| age > LOCK_STALE_AFTER)
            .unwrap_or(false),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sync::SyncState;

    #[tokio::test]
    async fn state_round_trips_and_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonSyncStateStore::new(dir.path());
        assert!(store.load().await.unwrap().is_none());

        let mut state = SyncState::new("example/repo", "main");
        state.record_success("abc");
        store.save(&state).await.unwrap();

        let mut loaded = store.load().await.unwrap().unwrap();
        // The store stamps the write time and the schema version, so the round
        // trip is compared on everything else and the stamp is asserted.
        assert!(loaded.saved_at.is_some());
        loaded.saved_at = None;
        assert_eq!(loaded, state);
        assert!(store.path().exists());
        assert!(!store.path().with_extension("json.tmp").exists());
    }

    #[tokio::test]
    async fn corrupt_state_is_an_error_not_a_silent_reset() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonSyncStateStore::new(dir.path());
        std::fs::write(store.path(), b"{ not json").unwrap();
        assert!(store.load().await.is_err());
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = SyncLock::acquire(dir.path()).unwrap();
        assert!(SyncLock::acquire(dir.path()).is_err());

        drop(first);
        assert!(SyncLock::acquire(dir.path()).is_ok());
    }

    #[test]
    fn dead_owner_lock_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOCK_FILE);
        std::fs::write(&path, b"999999999\n").unwrap();
        assert!(SyncLock::acquire(dir.path()).is_ok());
    }

    #[tokio::test]
    async fn state_is_readable_by_other_accounts() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = JsonSyncStateStore::new(dir.path());
        store.save(&SyncState::new("repo", "main")).await.unwrap();

        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o644,
            "`nod sync status` must be able to read the state as a normal user"
        );
    }
}
