//! typors editor crate.
//!
//! Owns the self-drawn GPUI element and everything that needs a window: focus,
//! caret, projection switching, keyboard/IME input, selections and undo. Geometry
//! lives in `typors-render`; the document model lives in `typors-core`.

pub mod capture;
pub mod ime;
pub mod media;
pub mod selection;
pub mod view;

pub use capture::{
    projection_invariants, DumpCapture, DumpSink, ImeSink, MediaProbeFn, MediaSink, PerfCapture,
    PerfSink, ProbeCapture, SelectSink,
};
pub use ime::{log_ime, MarkedText};
pub use selection::{HighlightRect, History, HistoryAction, Selection, select_probe};
pub use view::{
    byte_to_utf16, edit_log_enabled, escape_insert, insertion_range, key_bindings, mode_for,
    normalize_newlines, utf16_to_byte, CaretMove, CaretState, EditRecord, EditTrigger, MarkdownView,
    PerfSample,
};
