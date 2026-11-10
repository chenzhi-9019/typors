//! typors document model: one `ropey` buffer, a block AST, the span table and the
//! lossless source <-> render projection.
//!
//! This crate has no GPU or window dependency, so the mapping invariants are
//! testable headlessly.

pub mod doc;
pub mod marked;
pub mod sample;

pub use doc::{
    Block, BlockKind, Document, MarkerKind, RenderPos, Span, SpanKind, Style,
};
pub use marked::MarkedText;
