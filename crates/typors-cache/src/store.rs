//! The persistent render cache: root, index, per-document directories, entries.
//!
//! The format is deliberately simple and defect-tolerant. Every read failure is
//! a miss; every write is a temp file renamed over the target, so a crash or a
//! second typors instance can never leave a half-written entry that is trusted.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hash;

/// Entry families, each with its own directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Decoded image bitmap.
    Image,
    /// Rasterised chart / Mermaid image.
    Raster,
    /// Math SVG + metrics.
    Math,
    /// Per-block layout geometry.
    Layout,
}

impl EntryKind {
    pub fn dir(self) -> &'static str {
        match self {
            EntryKind::Image => "img",
            EntryKind::Raster => "raster",
            EntryKind::Math => "math",
            EntryKind::Layout => "layout",
        }
    }
}

const MAGIC: &[u8; 4] = b"TPC1";
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 4 + 8;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Index {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    documents: BTreeMap<String, IndexEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct IndexEntry {
    #[serde(default)]
    dir: String,
    #[serde(default)]
    last_used: u64,
    #[serde(default)]
    bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Meta {
    #[serde(default)]
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    hash: String,
    #[serde(default)]
    scale: f32,
    #[serde(default)]
    font_tag: String,
    #[serde(default)]
    created: u64,
    #[serde(default)]
    last_used: u64,
}

/// One document's cache directory.
#[derive(Clone, Debug)]
pub struct DocumentCache {
    root: PathBuf,
    dir_name: String,
    path: String,
}

/// Result of a garbage-collection pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub removed_documents: usize,
    pub freed_bytes: u64,
    pub total_bytes: u64,
}

/// One row of the cache status report.
#[derive(Clone, Debug)]
pub struct DocumentStatus {
    pub path: String,
    pub dir: String,
    pub bytes: u64,
    pub last_used: u64,
}

/// Whole-cache status for the status command and probe.
#[derive(Clone, Debug)]
pub struct CacheStatus {
    pub root: PathBuf,
    pub total_bytes: u64,
    pub documents: Vec<DocumentStatus>,
}

/// The cache root and its index.
pub struct CacheStore {
    root: PathBuf,
    index: Index,
    open: BTreeSet<String>,
}

impl CacheStore {
    /// Open (or create) the cache at root. Err means the cache is unusable for
    /// this session, which callers treat as "disabled", never as fatal.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        std::fs::create_dir_all(root.join("docs"))
            .map_err(|error| format!("cache root {}: {error}", root.display()))?;
        let index = read_index(&root).unwrap_or_default();
        Ok(Self {
            root,
            index,
            open: BTreeSet::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-adopt directories that exist on disk but are missing from the index
    /// (the index was deleted, or a previous run crashed before saving it).
    pub fn adopt_orphans(&mut self, now: u64) -> usize {
        let docs = self.root.join("docs");
        let Ok(entries) = std::fs::read_dir(&docs) else {
            return 0;
        };
        let known: BTreeSet<String> = self.index.documents.values().map(|e| e.dir.clone()).collect();
        let mut adopted = 0;
        for entry in entries.flatten() {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if known.contains(&dir_name) || !entry.path().is_dir() {
                continue;
            }
            let Some(meta) = read_meta(&entry.path()) else {
                continue;
            };
            if meta.path.is_empty() {
                continue;
            }
            let bytes = dir_size(&entry.path());
            self.index.documents.insert(
                meta.path,
                IndexEntry {
                    dir: dir_name,
                    last_used: if meta.last_used == 0 { now } else { meta.last_used },
                    bytes,
                },
            );
            adopted += 1;
        }
        if adopted > 0 {
            let _ = self.save_index();
        }
        adopted
    }

    /// Register (or re-open) a document's cache directory.
    pub fn register(&mut self, path: &str, now: u64) -> Result<DocumentCache, String> {
        if let Some(entry) = self.index.documents.get_mut(path) {
            let dir = self.root.join("docs").join(&entry.dir);
            if dir.join("meta.json").is_file() {
                entry.last_used = now;
                let dir_name = entry.dir.clone();
                let _ = self.save_index();
                self.open.insert(path.to_string());
                if let Some(meta) = read_meta(&dir) {
                    let _ = write_meta(
                        &dir,
                        &Meta {
                            last_used: now,
                            ..meta
                        },
                    );
                }
                return Ok(DocumentCache {
                    root: self.root.clone(),
                    dir_name,
                    path: path.to_string(),
                });
            }
            self.index.documents.remove(path);
        }

        let base = document_dir_name(path);
        let mut candidate = base.clone();
        let mut suffix = 1;
        loop {
            let dir = self.root.join("docs").join(&candidate);
            if !dir.exists() {
                break;
            }
            match read_meta(&dir) {
                Some(meta) if meta.path == path => break,
                _ => {
                    suffix += 1;
                    if suffix > 64 {
                        return Err(format!("cache: no free directory for {path}"));
                    }
                    candidate = format!("{base}-{suffix}");
                }
            }
        }

        let dir = self.root.join("docs").join(&candidate);
        for kind in [EntryKind::Image, EntryKind::Raster, EntryKind::Math, EntryKind::Layout] {
            std::fs::create_dir_all(dir.join(kind.dir()))
                .map_err(|error| format!("cache dir {}: {error}", dir.display()))?;
        }
        write_meta(
            &dir,
            &Meta {
                path: path.to_string(),
                size: 0,
                hash: String::new(),
                scale: 0.0,
                font_tag: String::new(),
                created: now,
                last_used: now,
            },
        )?;
        self.index.documents.insert(
            path.to_string(),
            IndexEntry {
                dir: candidate.clone(),
                last_used: now,
                bytes: 0,
            },
        );
        self.save_index()?;
        self.open.insert(path.to_string());
        Ok(DocumentCache {
            root: self.root.clone(),
            dir_name: candidate,
            path: path.to_string(),
        })
    }

    /// A document is no longer open; it becomes evictable.
    pub fn close_document(&mut self, path: &str) {
        self.open.remove(path);
    }

    /// Validate a document's cache against the file that was just read.
    pub fn validate(&self, path: &str, size: u64, hash: &str) -> bool {
        let Some(entry) = self.index.documents.get(path) else {
            return false;
        };
        let dir = self.root.join("docs").join(&entry.dir);
        match read_meta(&dir) {
            Some(meta) => meta.path == path && meta.size == size && meta.hash == hash,
            None => false,
        }
    }

    /// Record the identity of the file that was just opened successfully.
    pub fn note_document(
        &mut self,
        path: &str,
        size: u64,
        content_hash: &str,
        scale: f32,
        font_tag: &str,
        now: u64,
    ) -> Result<(), String> {
        let Some(entry) = self.index.documents.get(path) else {
            return Ok(());
        };
        let dir = self.root.join("docs").join(&entry.dir);
        let previous = read_meta(&dir).unwrap_or_default();
        write_meta(
            &dir,
            &Meta {
                path: path.to_string(),
                size,
                hash: content_hash.to_string(),
                scale,
                font_tag: font_tag.to_string(),
                created: previous.created,
                last_used: now,
            },
        )
    }

    /// A changed document invalidates its whole directory: cheaper and safer
    /// than diffing entry-by-entry after a large rewrite.
    pub fn invalidate_document(&mut self, path: &str) -> Result<(), String> {
        let Some(entry) = self.index.documents.get(path).cloned() else {
            return Ok(());
        };
        let dir = self.root.join("docs").join(&entry.dir);
        for kind in [EntryKind::Image, EntryKind::Raster, EntryKind::Math, EntryKind::Layout] {
            let _ = std::fs::remove_dir_all(dir.join(kind.dir()));
            std::fs::create_dir_all(dir.join(kind.dir()))
                .map_err(|error| format!("cache dir: {error}"))?;
        }
        if let Some(meta) = read_meta(&dir) {
            let cleared = Meta {
                size: 0,
                hash: String::new(),
                ..meta
            };
            let _ = write_meta(&dir, &cleared);
        }
        if let Some(index_entry) = self.index.documents.get_mut(path) {
            index_entry.bytes = 0;
        }
        self.save_index()
    }

    /// Evict least-recently-used document directories until the tree is under
    /// max_bytes. Open documents are never evicted.
    pub fn gc(&mut self, max_bytes: u64) -> GcReport {
        let mut total = 0u64;
        for path in self.index.documents.keys().cloned().collect::<Vec<_>>() {
            if let Some(entry) = self.index.documents.get_mut(&path) {
                let dir = self.root.join("docs").join(&entry.dir);
                entry.bytes = dir_size(&dir);
                total += entry.bytes;
            }
        }
        let mut removed = 0;
        let mut freed = 0;
        while total > max_bytes {
            let victim = self
                .index
                .documents
                .iter()
                .filter(|(path, _)| !self.open.contains(*path))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(path, entry)| (path.clone(), entry.dir.clone(), entry.bytes));
            let Some((path, dir_name, bytes)) = victim else {
                break;
            };
            let _ = std::fs::remove_dir_all(self.root.join("docs").join(&dir_name));
            self.index.documents.remove(&path);
            removed += 1;
            freed += bytes;
            total = total.saturating_sub(bytes);
        }
        if removed > 0 {
            let _ = self.save_index();
        }
        GcReport {
            removed_documents: removed,
            freed_bytes: freed,
            total_bytes: total,
        }
    }

    /// Current usage and per-document rows.
    pub fn status(&self) -> CacheStatus {
        let mut total = 0;
        let mut documents = Vec::with_capacity(self.index.documents.len());
        for (path, entry) in &self.index.documents {
            let dir = self.root.join("docs").join(&entry.dir);
            let bytes = dir_size(&dir);
            total += bytes;
            documents.push(DocumentStatus {
                path: path.clone(),
                dir: entry.dir.clone(),
                bytes,
                last_used: entry.last_used,
            });
        }
        documents.sort_by(|a, b| b.last_used.cmp(&a.last_used).then(a.path.cmp(&b.path)));
        CacheStatus {
            root: self.root.clone(),
            total_bytes: total,
            documents,
        }
    }

    /// Remove every cached document. Open handles keep working from memory.
    pub fn clear(&mut self) -> Result<(), String> {
        let docs = self.root.join("docs");
        std::fs::remove_dir_all(&docs).map_err(|error| format!("clear cache: {error}"))?;
        std::fs::create_dir_all(&docs).map_err(|error| format!("clear cache: {error}"))?;
        self.index.documents.clear();
        self.open.clear();
        self.save_index()
    }

    fn save_index(&self) -> Result<(), String> {
        let path = self.root.join("index.json");
        let json = serde_json::to_vec_pretty(&self.index)
            .map_err(|error| format!("cache index: {error}"))?;
        write_atomic(&path, &json)
    }
}

impl DocumentCache {
    /// Canonical absolute path this directory belongs to.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Directory name under the cache root.
    pub fn dir_name(&self) -> &str {
        &self.dir_name
    }

    pub fn dir(&self) -> PathBuf {
        self.root.join("docs").join(&self.dir_name)
    }

    /// Read one entry; every failure is None.
    pub fn read(&self, kind: EntryKind, key: &str) -> Option<Vec<u8>> {
        read_entry(&self.entry_path(kind, key))
    }

    /// Write one entry atomically.
    pub fn write(&self, kind: EntryKind, key: &str, payload: &[u8]) -> Result<(), String> {
        let path = self.entry_path(kind, key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("cache dir: {error}"))?;
        }
        write_entry(&path, payload)
    }

    pub fn remove(&self, kind: EntryKind, key: &str) {
        let _ = std::fs::remove_file(self.entry_path(kind, key));
    }

    pub fn entry_path(&self, kind: EntryKind, key: &str) -> PathBuf {
        let name = if key.is_empty() {
            "empty".to_string()
        } else {
            key.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect()
        };
        self.dir().join(kind.dir()).join(format!("{name}.bin"))
    }

    /// Bytes currently used by this document.
    pub fn used_bytes(&self) -> u64 {
        dir_size(&self.dir())
    }
}

/// Deterministic directory name: slug plus 16 hex characters of the path hash.
pub fn document_dir_name(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());
    let mut slug: String = stem
        .chars()
        .take(40)
        .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
        .collect();
    if slug.is_empty() {
        slug = "document".to_string();
    }
    format!("{slug}-{}", hash::hash_path(Path::new(path)))
}

/// Key for a resource from its identifying bytes (source bytes, fence text, ...).
pub fn resource_key(parts: &[u8]) -> String {
    hash::hex16(hash::fnv1a64(parts))
}

/// Key for a resource from identifying bytes plus numeric fields.
pub fn keyed(parts: &[u8], numbers: &[u64]) -> String {
    let mut buffer = Vec::with_capacity(parts.len() + numbers.len() * 8);
    buffer.extend_from_slice(parts);
    for number in numbers {
        buffer.extend_from_slice(&number.to_le_bytes());
    }
    hash::hex16(hash::fnv1a64(&buffer))
}

fn index_path(root: &Path) -> PathBuf {
    root.join("index.json")
}

fn read_index(root: &Path) -> Option<Index> {
    let bytes = std::fs::read(index_path(root)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("meta.json")
}

fn read_meta(dir: &Path) -> Option<Meta> {
    let bytes = std::fs::read(meta_path(dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_meta(dir: &Path, meta: &Meta) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(meta).map_err(|error| format!("cache meta: {error}"))?;
    write_atomic(&meta_path(dir), &json)
}

/// Temp file + rename, so a reader never sees a partial file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| format!("cache: {error}"))?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "entry".to_string()),
        std::process::id(),
        n
    ));
    {
        let mut file = std::fs::File::create(&temp).map_err(|error| format!("cache: {error}"))?;
        file.write_all(bytes).map_err(|error| format!("cache: {error}"))?;
        file.sync_all().ok();
    }
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("cache: {error}")
    })
}

fn write_entry(path: &Path, payload: &[u8]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&hash::fnv1a64(payload).to_le_bytes());
    bytes.extend_from_slice(payload);
    write_atomic(path, &bytes)
}

fn read_entry(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return None;
    }
    let len = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let checksum = u64::from_le_bytes([
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
    ]);
    let payload = &bytes[HEADER_LEN..];
    if payload.len() != len || hash::fnv1a64(payload) != checksum {
        return None;
    }
    Some(payload.to_vec())
}

/// Recursive size of a directory tree, ignoring failures.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

/// Wall clock in seconds for callers that do not inject one.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("typors-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn directory_names_are_stable_and_distinct() {
        let a = document_dir_name(r"C:\a\notes.md");
        let b = document_dir_name(r"C:\b\notes.md");
        assert_eq!(a, document_dir_name(r"C:\a\notes.md"));
        assert_ne!(a, b);
        assert!(a.starts_with("notes-"));
        assert_eq!(a.len(), "notes-".len() + 16);
        // Case differences are distinct keys.
        assert_ne!(document_dir_name(r"C:\A\Notes.md"), a);
    }

    #[test]
    fn register_read_write_round_trip() {
        let root = temp_root("roundtrip");
        let mut store = CacheStore::open(&root).unwrap();
        let cache = store.register(r"C:\work\notes.md", 100).unwrap();
        let key = resource_key(b"image-bytes");
        assert!(cache.read(EntryKind::Image, &key).is_none());
        cache.write(EntryKind::Image, &key, b"payload").unwrap();
        assert_eq!(cache.read(EntryKind::Image, &key).as_deref(), Some(&b"payload"[..]));
        assert!(cache.entry_path(EntryKind::Image, &key).starts_with(cache.dir()));
        assert!(cache.dir().join("img").is_dir());
        assert!(cache.used_bytes() > 0);

        // Reopening the same root finds the same directory.
        let mut reopened = CacheStore::open(&root).unwrap();
        let again = reopened.register(r"C:\work\notes.md", 200).unwrap();
        assert_eq!(again.dir_name(), cache.dir_name());
        assert_eq!(again.read(EntryKind::Image, &key).as_deref(), Some(&b"payload"[..]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_files_with_the_same_name_get_their_own_directories() {
        let root = temp_root("same-name");
        let mut store = CacheStore::open(&root).unwrap();
        let a = store.register(r"C:\a\notes.md", 1).unwrap();
        let b = store.register(r"C:\b\notes.md", 1).unwrap();
        assert_ne!(a.dir_name(), b.dir_name());
        a.write(EntryKind::Math, "k", b"a").unwrap();
        assert!(b.read(EntryKind::Math, "k").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_and_truncated_entries_are_misses() {
        let root = temp_root("corrupt");
        let mut store = CacheStore::open(&root).unwrap();
        let cache = store.register(r"C:\x\doc.md", 1).unwrap();
        cache.write(EntryKind::Layout, "abc", b"0123456789").unwrap();
        let path = cache.entry_path(EntryKind::Layout, "abc");

        // Truncated payload.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 2);
        std::fs::write(&path, &bytes).unwrap();
        assert!(cache.read(EntryKind::Layout, "abc").is_none());

        // Garbage header.
        std::fs::write(&path, b"not a cache entry").unwrap();
        assert!(cache.read(EntryKind::Layout, "abc").is_none());

        // Mismatched checksum.
        let mut good = Vec::new();
        good.extend_from_slice(MAGIC);
        good.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        good.extend_from_slice(&4u32.to_le_bytes());
        good.extend_from_slice(&0u64.to_le_bytes());
        good.extend_from_slice(b"data");
        std::fs::write(&path, &good).unwrap();
        assert!(cache.read(EntryKind::Layout, "abc").is_none());

        // A rewrite recovers the entry.
        cache.write(EntryKind::Layout, "abc", b"fresh").unwrap();
        assert_eq!(cache.read(EntryKind::Layout, "abc").as_deref(), Some(&b"fresh"[..]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validation_tracks_size_and_content_hash() {
        let root = temp_root("validate");
        let mut store = CacheStore::open(&root).unwrap();
        let path = r"C:\v\doc.md";
        store.register(path, 1).unwrap();
        assert!(!store.validate(path, 10, "hash-a"));
        store.note_document(path, 10, "hash-a", 1.0, "font", 2).unwrap();
        assert!(store.validate(path, 10, "hash-a"));
        assert!(!store.validate(path, 11, "hash-a"));
        assert!(!store.validate(path, 10, "hash-b"));
        store.invalidate_document(path).unwrap();
        assert!(!store.validate(path, 10, "hash-a"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gc_evicts_lru_but_spares_open_documents() {
        let root = temp_root("gc");
        let mut store = CacheStore::open(&root).unwrap();
        let old = store.register(r"C:\g\old.md", 10).unwrap();
        let new = store.register(r"C:\g\new.md", 20).unwrap();
        let open_doc = store.register(r"C:\g\open.md", 5).unwrap();
        for cache in [&old, &new, &open_doc] {
            cache.write(EntryKind::Image, "k", &[7u8; 4096]).unwrap();
        }
        // old.md and new.md were closed; open.md is still open.
        store.close_document(old.path());
        store.close_document(new.path());
        // open.md is open, old.md is the LRU candidate.
        let before = store.status().total_bytes;
        assert!(before > 4096);
        let report = store.gc(before - 1);
        assert_eq!(report.removed_documents, 1);
        assert!(old.dir().exists() == false || !old.dir().join("img").exists());
        assert!(new.dir().exists());
        assert!(open_doc.dir().exists());
        // Under the cap: nothing else is removed.
        let report = store.gc(u64::MAX);
        assert_eq!(report.removed_documents, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_reports_documents_and_clear_removes_them() {
        let root = temp_root("status");
        let mut store = CacheStore::open(&root).unwrap();
        let cache = store.register(r"C:\s\doc.md", 42).unwrap();
        cache.write(EntryKind::Raster, "r", b"12345678").unwrap();
        let status = store.status();
        assert_eq!(status.documents.len(), 1);
        assert!(status.total_bytes > 0);
        assert_eq!(status.documents[0].dir, cache.dir_name());
        assert!(status.documents[0].path.contains("doc.md"));

        store.clear().unwrap();
        assert_eq!(store.status().documents.len(), 0);
        assert!(!cache.dir().exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn orphans_are_adopted() {
        let root = temp_root("orphan");
        let mut store = CacheStore::open(&root).unwrap();
        let cache = store.register(r"C:\o\doc.md", 1).unwrap();
        cache.write(EntryKind::Image, "k", b"x").unwrap();
        // Simulate a lost index.
        std::fs::remove_file(root.join("index.json")).unwrap();
        let mut store = CacheStore::open(&root).unwrap();
        assert_eq!(store.adopt_orphans(3), 1);
        assert!(store.validate(r"C:\o\doc.md", 0, "") || store.status().documents.len() == 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
