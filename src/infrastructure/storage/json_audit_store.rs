//! JSON audit store adapter: appends/reads deployment outcomes as a JSON
//! array on disk (ADR-003 observability).
//!
//! The adapter owns a single backing file (a JSON array of `AuditEntry`).
//! `record` appends; `entries` reads newest-first, optional host filter and
//! count cap. A missing file reads as an empty history, not an error.

use async_trait::async_trait;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::audit::AuditEntry;
use crate::domain::errors::NodError;
use crate::domain::ports::audit_store::AuditStorePort;

/// Audit history backed by a JSON array file.
pub struct JsonAuditStore {
    path: PathBuf,
}

impl JsonAuditStore {
    /// Builds a store at the platform history path
    /// (`$HOME/.local/share/nod/history.json`), falling back to a `.nod/`
    /// directory in the current working directory when `$HOME` is unset.
    pub fn new() -> Self {
        Self {
            path: Self::default_path(),
        }
    }
}

impl Default for JsonAuditStore {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonAuditStore {
    /// Builds a store pointed at an explicit `path` (for tests and callers
    /// that override the location).
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// The default history file location.
    fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        if home == "." {
            PathBuf::from(".").join(".nod").join("history.json")
        } else {
            PathBuf::from(&home)
                .join(".local")
                .join("share")
                .join("nod")
                .join("history.json")
        }
    }

    /// Reads the whole history (empty when the file is absent).
    fn read(&self) -> Result<Vec<AuditEntry>, NodError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&self.path).map_err(|e| {
            NodError::config(format!(
                "cannot read history {}: {}",
                self.path.display(),
                e
            ))
        })?;
        serde_json::from_str::<Vec<AuditEntry>>(&raw).map_err(|e| {
            NodError::config(format!("corrupt history {}: {}", self.path.display(), e))
        })
    }

    /// Writes `entries` back to the backing file, creating parent dirs.
    fn write(&self, entries: Vec<AuditEntry>) -> Result<(), NodError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                NodError::config(format!(
                    "cannot create history dir {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        let raw = serde_json::to_string(&entries)
            .map_err(|e| NodError::config(format!("cannot serialise history: {}", e)))?;
        std::fs::write(&self.path, raw).map_err(|e| {
            NodError::config(format!(
                "cannot write history {}: {}",
                self.path.display(),
                e
            ))
        })?;
        Ok(())
    }

    /// Appends one entry at the current wall-clock second.
    ///
    /// Persistence failures (a read error, corrupt existing history, or a
    /// write error) are propagated rather than swallowed, so the caller can
    /// surface that the deployment outcome was not recorded (ADR-003). A
    /// corrupt existing history is reported as an error and is never
    /// silently truncated and replaced with a fresh single entry.
    fn append(&self, entry: AuditEntry) -> Result<(), NodError> {
        let mut all = self.read()?;
        all.push(entry);
        self.write(all)
    }

    /// The current unix time in whole seconds.
    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

#[async_trait]
impl AuditStorePort for JsonAuditStore {
    async fn record(&self, host_name: &str, outcome: &str) -> Result<(), NodError> {
        self.append(AuditEntry::new(host_name, outcome, Self::now_epoch()))
    }

    async fn entries(
        &self,
        host: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEntry>, NodError> {
        let mut all = self.read()?;

        if let Some(host_filter) = &host {
            all.retain(|e| &e.host_name == host_filter);
        }

        // Newest-first (the file is append-ordered oldest-first).
        let mut newest = Vec::<AuditEntry>::with_capacity(all.len());
        while let Some(entry) = all.pop() {
            newest.push(entry);
        }
        let all = newest;

        if let Some(limit) = limit {
            let mut capped = Vec::<AuditEntry>::with_capacity(limit);
            for (taken, entry) in all.into_iter().enumerate() {
                if taken >= limit {
                    break;
                }
                capped.push(entry);
            }
            Ok(capped)
        } else {
            Ok(all)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn missing_file_reads_as_empty_not_an_error() {
        let dir = tempdir().unwrap();
        let store = JsonAuditStore::at(dir.path().join("history.json"));
        let entries = store.entries(None, None).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn recorded_entries_round_trip_and_sort_newest_first() {
        let dir = tempdir().unwrap();
        let store = JsonAuditStore::at(dir.path().join("history.json"));
        // record() stamps epoch seconds internally, so entries come back in
        // the chronological append order regardless of equal timestamps.
        store.record("jello", "completed").await.unwrap();
        store.record("atlas", "failed").await.unwrap();
        store.record("jello", "rolled_back").await.unwrap();

        let all = store.entries(None, None).await.unwrap();
        assert_eq!(all.len(), 3);
        // Newest first: the last recorded host leads the list.
        assert_eq!(all[0].host_name, "jello");
        assert_eq!(all[0].outcome, "rolled_back");
        assert_eq!(all[2].host_name, "jello");
        assert_eq!(all[2].outcome, "completed");
    }

    #[tokio::test]
    async fn entries_narrow_by_host_filter() {
        let dir = tempdir().unwrap();
        let store = JsonAuditStore::at(dir.path().join("history.json"));
        store.record("jello", "completed").await.unwrap();
        store.record("atlas", "completed").await.unwrap();

        let jello = store
            .entries(Some("jello".to_string()), None)
            .await
            .unwrap();
        assert_eq!(jello.len(), 1);
        assert_eq!(jello[0].host_name, "jello");
    }

    #[tokio::test]
    async fn record_propagates_a_write_failure() {
        let dir = tempdir().unwrap();
        // Make the parent path a regular file so directory creation fails,
        // surfacing a persistence error instead of silently dropping the
        // entry (AC-B2.2 / AC-B2.4b).
        let blocker = dir.path().join("not_a_dir");
        std::fs::write(&blocker, "x").unwrap();
        let store = JsonAuditStore::at(blocker.join("history.json"));
        let err = store.record("jello", "completed").await.unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
    }

    #[tokio::test]
    async fn record_propagates_corrupt_history_instead_of_truncating() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, "not valid json {").unwrap();
        let store = JsonAuditStore::at(path.clone());
        let err = store.record("jello", "completed").await.unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        // The corrupt history must not be overwritten with a fresh entry
        // (AC-B2.3): prior records are preserved on disk.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("not valid json"));
    }

    #[tokio::test]
    async fn limit_caps_the_newest_entries() {
        let dir = tempdir().unwrap();
        let store = JsonAuditStore::at(dir.path().join("history.json"));
        store.record("a", "ok").await.unwrap();
        store.record("b", "ok").await.unwrap();
        store.record("c", "ok").await.unwrap();

        let capped = store.entries(None, Some(2)).await.unwrap();
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].host_name, "c");
        assert_eq!(capped[1].host_name, "b");
    }
}
