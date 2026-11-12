//! Runtime wiring for per-file reading positions: identify a document, restore
//! its position, record it on close and on a periodic checkpoint.

use std::path::PathBuf;

use gpui_kit::gpui::{App, Entity};
use typors_cache::session::{SessionStore, content_hash};
use typors_editor::view::MarkdownView;

/// What identifies one document for the position store.
#[derive(Clone, Debug)]
pub struct DocumentId {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

impl DocumentId {
    /// Canonical absolute path where the platform can produce one, the plain
    /// absolute path otherwise. Untitled documents have no identity.
    pub fn of(source: &Option<PathBuf>, text: &str) -> Option<Self> {
        let source = source.as_ref()?;
        let canonical = std::fs::canonicalize(source).unwrap_or_else(|_| source.clone());
        Some(Self {
            path: canonical.to_string_lossy().to_string(),
            hash: content_hash(text),
            size: text.len() as u64,
        })
    }
}

/// Apply a stored position to a freshly created view, before its first layout.
/// A changed file is clamped, never guessed.
pub fn restore(view: &Entity<MarkdownView>, id: &DocumentId, cx: &mut App) {
    let store = SessionStore::load();
    let Some(record) = store.matches(&id.path, id.size, &id.hash) else {
        return;
    };
    let _ = view.update(cx, |view, _cx| {
        let caret = record.caret.min(view.doc().len_bytes());
        view.set_caret(caret);
        view.set_scroll_offset_y(record.scroll.max(0.0));
    });
}

/// Capture and persist the current position. Failures are silent: a read-only
/// profile must never interrupt open, save or close.
pub fn record(view: &Entity<MarkdownView>, id: &DocumentId, cx: &mut App) -> bool {
    let (caret, scroll) = view.update(cx, |view, _cx| {
        (view.caret().unwrap_or(0), view.scroll_offset_y())
    });
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut store = SessionStore::load();
    store.record(&id.path, caret, scroll, id.size, &id.hash, now);
    store.prune(now);
    store.save().is_ok()
}

/// Write a checkpoint every 30 seconds while the view lives, so an abnormal
/// termination loses at most one interval of scrolling.
pub fn spawn_checkpoint(view: Entity<MarkdownView>, id: DocumentId, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            let alive = cx.update(|cx| record(&view, &id, cx));
            if !alive {
                break;
            }
        }
    })
    .detach();
}
