//! typors M0 hybrid-editor spike.
//!
//! Layering, bottom up:
//!
//! * [`doc`] — one `ropey` buffer, a block AST and the span table (H3).
//! * [`render`] — the render projection's geometry: which fragment is painted
//!   where, block boxes, byte <-> x, and the one-block source projection. No
//!   `Window`, so it is testable headlessly.
//! * [`view`] — the self-drawn GPUI `Element` that measures through the window
//!   text system and paints [`render::DocLayout`], plus the caret and the single
//!   text-mutation entry point.
//! * [`ime`] — the Windows input-method channel (H2): the composition buffer that
//!   never reaches the rope, the callback log for the manual pass, and the
//!   `--dump-ime` probe.
//! * [`selection`] — group 5: the selection (two source offsets), its highlight
//!   geometry, the undo/redo history, and the `--dump-select` probe.
//! * [`perf`] — group 7: the frame-cost sweep (`--dump-perf`): viewport culling
//!   and the two caches, measured per frame, plus the geometry-equality checks
//!   that prove the optimisations change the cost and not the result.
//! * [`probe`] — the programmatic stand-in for the manual checks of groups 3/4.

pub mod doc;
pub mod ime;
pub mod perf;
pub mod probe;
pub mod render;
pub mod sample;
pub mod selection;
pub mod view;

pub use doc::{Block, BlockKind, Document, MarkerKind, RenderPos, Span, SpanKind, Style};
pub use ime::{log_ime, MarkedText};
pub use perf::{PerfCapture, PerfSink, percentile};
pub use probe::ProbeCapture;
pub use render::{
    ApproxMetrics, BlockBox, BlockCache, BlockDecor, BlockMode, DocLayout, DrawSpan, EditorTheme,
    LayoutMode, Metrics, RenderHit, ShapeCache, Viewport, WindowMetrics,
};
pub use selection::{HighlightRect, History, HistoryAction, Selection, select_probe};
pub use view::{
    byte_to_utf16, edit_log_enabled, escape_insert, insertion_range, key_bindings, mode_for,
    normalize_newlines, utf16_to_byte, CaretMove, CaretState, DumpCapture, DumpSink, EditRecord,
    EditTrigger, ImeSink, MarkdownView, PerfSample, SelectSink,
};
