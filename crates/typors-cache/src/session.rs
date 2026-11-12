//! Per-file reading positions: where the caret and the scroll were when a
//! document was last closed, keyed by the document's canonical absolute path.
//!
//! Deliberately separate from the render cache: clearing the cache must not lose
//! a reading position, and a read-only executable directory must not disable it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How long a record for a deleted file is kept.
pub const DEFAULT_RETENTION_DAYS: u64 = 30;
/// Hard cap on stored records, evicting the least recently used.
pub const DEFAULT_MAX_ENTRIES: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Record {
    #[serde(default)]
    pub caret: usize,
    #[serde(default)]
    pub scroll: f32,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub updated: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    documents: BTreeMap<String, Record>,
}

/// The position store.
#[derive(Clone, Debug, Default)]
pub struct SessionStore {
    path: PathBuf,
    file: File,
    /// Set when loading had to discard a corrupt file; the caller may surface it.
    warning: Option<String>,
}

impl SessionStore {
    /// Platform path: %APPDATA%\typors\sessions.json.
    /// TYPORS_SESSIONS_PATH overrides it for tests and probes.
    pub fn path() -> PathBuf {
        if let Ok(path) = std::env::var("TYPORS_SESSIONS_PATH") {
            return PathBuf::from(path);
        }
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("typors").join("sessions.json")
    }

    /// Load from the platform path, tolerating every failure.
    pub fn load() -> Self {
        Self::load_from(Self::path())
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut store = SessionStore {
            path,
            file: File::default(),
            warning: None,
        };
        match std::fs::read(&store.path) {
            Ok(bytes) => match serde_json::from_slice::<File>(&bytes) {
                Ok(file) => store.file = file,
                Err(error) => store.warning = Some(format!("sessions.json is corrupt: {error}")),
            },
            Err(_) => {}
        }
        store.file.version = 1;
        store
    }

    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn file_path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.file.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.file.documents.is_empty()
    }

    pub fn get(&self, document: &str) -> Option<&Record> {
        self.file.documents.get(document)
    }

    /// Record where the user was. The identity fields let the next open tell an
    /// unchanged file from an edited one.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        document: &str,
        caret: usize,
        scroll: f32,
        size: u64,
        hash: &str,
        now: u64,
    ) {
        self.file.documents.insert(
            document.to_string(),
            Record {
                caret,
                scroll,
                size,
                hash: hash.to_string(),
                updated: now,
            },
        );
    }

    /// The record, but only when it still describes the file that was just read.
    pub fn matches(&self, document: &str, size: u64, hash: &str) -> Option<Record> {
        let record = self.file.documents.get(document)?;
        (record.size == size && record.hash == hash).then(|| record.clone())
    }

    /// Drop records for files that no longer exist after the retention window,
    /// then cap the table by least-recently-used.
    pub fn prune(&mut self, now: u64) -> usize {
        let retention = DEFAULT_RETENTION_DAYS * 24 * 3600;
        let before = self.file.documents.len();
        self.file.documents.retain(|path, record| {
            if now.saturating_sub(record.updated) <= retention {
                return true;
            }
            Path::new(path).exists()
        });
        while self.file.documents.len() > DEFAULT_MAX_ENTRIES {
            let oldest = self
                .file
                .documents
                .iter()
                .min_by_key(|(_, record)| record.updated)
                .map(|(path, _)| path.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.file.documents.remove(&oldest);
        }
        before - self.file.documents.len()
    }

    /// Atomic write (temp file + rename). A failure is returned, never fatal.
    pub fn save(&mut self) -> Result<(), String> {
        let Some(parent) = self.path.parent() else {
            return Err("sessions path has no parent".to_string());
        };
        std::fs::create_dir_all(parent).map_err(|error| format!("sessions dir: {error}"))?;
        self.file.version = 1;
        let json = serde_json::to_vec_pretty(&self.file)
            .map_err(|error| format!("sessions: {error}"))?;
        let temp = parent.join(format!(".sessions.{}.tmp", std::process::id()));
        std::fs::write(&temp, &json).map_err(|error| format!("sessions write: {error}"))?;
        std::fs::rename(&temp, &self.path).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            format!("sessions rename: {error}")
        })
    }

    pub fn clear(&mut self) {
        self.file.documents.clear();
    }
}

/// Stable hash of a document's bytes for identity checks.
pub fn content_hash(text: &str) -> String {
    crate::hash::hex16(crate::hash::fnv1a64(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("typors-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("sessions.json")
    }

    #[test]
    fn round_trip_and_restore_match() {
        let path = temp("roundtrip");
        let document = r"C:\work\notes.md";
        let mut store = SessionStore::load_from(&path);
        store.record(document, 1234, 880.5, 4567, "abcd", 100);
        store.save().unwrap();
        let loaded = SessionStore::load_from(&path);
        let record = loaded.get(document).expect("record");
        assert_eq!(record.caret, 1234);
        assert_eq!(record.scroll, 880.5);
        assert_eq!(record.size, 4567);
        assert!(loaded.matches(document, 4567, "abcd").is_some());
        assert!(loaded.matches(document, 4567, "other").is_none());
        assert!(loaded.matches(document, 9999, "abcd").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn corrupt_file_is_ignored_with_a_warning() {
        let path = temp("corrupt");
        std::fs::write(&path, b"{not json").unwrap();
        let mut store = SessionStore::load_from(&path);
        assert!(store.is_empty());
        assert!(store.warning().is_some());
        store.record(r"C:\a.md", 1, 0.0, 2, "h", 3);
        store.save().unwrap();
        assert_eq!(SessionStore::load_from(&path).len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pruning_drops_expired_missing_files_and_caps_the_table() {
        let path = temp("prune");
        let mut store = SessionStore::load_from(&path);
        let now = 1_000_000_000u64;
        store.record(
            r"C:\gone.md",
            0,
            0.0,
            0,
            "",
            now - DEFAULT_RETENTION_DAYS * 24 * 3600 - 10,
        );
        store.record(r"C:\recent.md", 1, 0.0, 0, "", now - 10);
        let removed = store.prune(now);
        assert_eq!(removed, 1);
        assert!(store.get(r"C:\gone.md").is_none());
        assert!(store.get(r"C:\recent.md").is_some());

        for index in 0..DEFAULT_MAX_ENTRIES + 5 {
            store.record(
                &format!(r"C:\f{index}.md"),
                0,
                0.0,
                0,
                "",
                now + index as u64,
            );
        }
        store.prune(now + 1_000);
        assert_eq!(store.len(), DEFAULT_MAX_ENTRIES);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unwritable_path_returns_an_error_instead_of_panicking() {
        let mut store = SessionStore::load_from(r"Z:\definitely\not\here\sessions.json");
        store.record(r"C:\a.md", 0, 0.0, 0, "", 0);
        assert!(store.save().is_err());
    }

    #[test]
    fn content_hash_is_stable_and_distinguishes_edits() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
    }
}
