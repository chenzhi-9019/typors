//! typors programmatic probes: the `--dump-*` transcripts and assertions.
//!
//! The probes drive the same [`typors_editor::view::MarkdownView`] the window
//! paints, then report what they measured. They are part of the product binary on
//! purpose: after a change, run them and compare.

#![forbid(unsafe_code)]

pub mod media;
pub mod perf;
pub mod probe;

pub use media::media_probe;
pub use typors_editor::{PerfCapture, PerfSink, ProbeCapture};
