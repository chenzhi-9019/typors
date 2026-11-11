//! Shared probe/capture values and the projection invariants.
//!
//! The sinks are plain `Arc<Mutex<Option<..>>>` values: the view fills one from
//! inside a painted frame, and the command-line probes read it afterwards. Keeping
//! the types here (rather than in the probes crate) means the editor never depends
//! on the probe runners, while both still agree on the shape of a report.

use std::sync::{Arc, Mutex};

use gpui_kit::gpui::Window;
use typors_core::BlockKind;
use typors_render::{BlockMode, DocLayout, SOURCE_SPAN};

use crate::view::MarkdownView;

/// What a probe run produced: a human-readable transcript plus pass/fail checks.
#[derive(Clone)]
pub struct ProbeCapture {
    pub report: String,
    pub checks: Vec<(String, bool, String)>,
}

/// What `--dump-perf` collects from the frame it ran in.
#[derive(Clone)]
pub struct PerfCapture {
    pub report: String,
    pub checks: Vec<(String, bool, String)>,
}

pub type PerfSink = Arc<Mutex<Option<PerfCapture>>>;

/// The `--dump-perf` sweep, injected by the application so the editor crate does
/// not depend on the probe crate. It runs inside a painted frame, where a
/// `Window` (and therefore the text system) exists.
pub type PerfProbeFn = fn(&mut MarkdownView, &mut Window) -> PerfCapture;

/// What a `--dump-layout` run captures from the frame it actually painted.
#[derive(Clone)]
pub struct DumpCapture {
    pub layout: DocLayout,
    pub report: String,
    pub checks: Vec<(String, bool, String)>,
    /// A second layout pass in the same frame, with the text system warm.
    pub remeasure_ms: f64,
    /// A third layout pass in the same frame, through the layout caches.
    pub cached_ms: f64,
}

pub type DumpSink = Arc<Mutex<Option<DumpCapture>>>;

/// Where `--dump-ime` collects the assertions it ran against a painted frame.
pub type ImeSink = Arc<Mutex<Option<ProbeCapture>>>;

/// Where `--dump-select` collects the assertions it ran against a painted frame.
pub type SelectSink = Arc<Mutex<Option<ProbeCapture>>>;

/// Where `--dump-media` collects the rich-content assertions.
pub type MediaSink = Arc<Mutex<Option<ProbeCapture>>>;

/// The `--dump-media` probe, injected by the application for the same reason as
/// [`PerfProbeFn`]: the probe lives in the probe crate.
pub type MediaProbeFn = fn(&MarkdownView) -> ProbeCapture;

/// Byte offsets at every char boundary of `text`, including `text.len()`.
fn boundaries(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    out.push(text.len());
    out
}

pub(crate) fn source_block_indices(layout: &DocLayout) -> Vec<usize> {
    layout
        .blocks
        .iter()
        .filter(|b| b.mode == BlockMode::Source)
        .map(|b| b.block_idx)
        .collect()
}

/// The invariants that must hold after *every* edit: exactly one source block,
/// it holds the caret, its painted text is the buffer slice, every other block is
/// rendered from `Block::render`, and display <-> source round-trips everywhere.
///
/// Shared by the edit/IME/selection probes so undo/redo cannot quietly weaken
/// these invariants in one probe but not another.
pub fn projection_invariants(
    view: &MarkdownView,
    what: &str,
    failures: &mut Vec<String>,
) -> String {
    let mut out = String::new();
    let Some(layout) = view.layout() else {
        failures.push(format!("{what}: no layout"));
        return out;
    };
    let caret_block = view.caret_block();
    let sources = source_block_indices(layout);
    out.push_str(&format!(
        "   [{what}] caret src={:?} in block {:?}; source blocks {sources:?}\n",
        view.caret(),
        caret_block
    ));
    match (caret_block, sources.as_slice()) {
        // Live preview: a rich chart/mermaid fence or a caret touching an inline
        // delimiter is source; every other block is rendered.
        (Some(block), [only])
            if block == *only
                && (view.doc().block_is_rich_fence(*only)
                    || view
                        .caret()
                        .is_some_and(|caret| view.doc().caret_touches_inline_marker(caret))) => {}
        // No caret or caret in an ordinary block: everything is rendered.
        (_, []) => {}
        (Some(block), other) => failures.push(format!(
            "{what}: caret in block {block}, unexpected source blocks {other:?}"
        )),
        (None, other) => failures.push(format!(
            "{what}: no caret, yet blocks {other:?} are projected as source"
        )),
    }
    let doc_text = view.doc().text();
    for block in &layout.blocks {
        let painted = block.painted_text();
        let displayed: String = block
            .display_text()
            .chars()
            .filter(|c| *c != '\n')
            .collect();
        if painted != displayed {
            failures.push(format!(
                "{what}: block {} painted {painted:?} != displayed {displayed:?}",
                block.block_idx
            ));
        }
        match block.mode {
            BlockMode::Source => {
                let expected = doc_text.get(block.src.clone()).unwrap_or_default();
                if block.display_text() != expected {
                    failures.push(format!(
                        "{what}: block {} source projection is not the buffer slice",
                        block.block_idx
                    ));
                }
                for span in &block.spans {
                    if span.span_idx != SOURCE_SPAN {
                        failures.push(format!(
                            "{what}: block {} source fragment claims span {}",
                            block.block_idx, span.span_idx
                        ));
                    }
                }
            }
            BlockMode::Rendered => {
                if block.display_text() != block.render {
                    failures.push(format!(
                        "{what}: block {} is rendered but not from Block::render",
                        block.block_idx
                    ));
                }
                if matches!(block.kind, BlockKind::Heading { .. }) && painted.contains('#') {
                    failures.push(format!(
                        "{what}: rendered heading {} paints '#'",
                        block.block_idx
                    ));
                }
            }
        }
        for local in boundaries(block.display_text()) {
            let Some(src) = layout.src_for_display_byte(view.doc(), block.block_idx, local) else {
                failures.push(format!(
                    "{what}: block {} byte {local} has no source offset",
                    block.block_idx
                ));
                continue;
            };
            if !view.doc().is_char_boundary(src) {
                failures.push(format!(
                    "{what}: block {} byte {local} -> src {src} is mid-char",
                    block.block_idx
                ));
                continue;
            }
            // The projection pair is not a bijection on block boundaries: the
            // byte just past a block belongs (canonically) to the block that
            // starts there. What must hold - because the caret *is* a source
            // offset - is that the round trip `src -> (block, byte) -> src` is
            // stable wherever it lands.
            let Some((back_block, back_byte)) = layout.display_byte_for_src(view.doc(), src) else {
                failures.push(format!(
                    "{what}: block {} byte {local} -> src {src} does not project back",
                    block.block_idx
                ));
                continue;
            };
            match layout.src_for_display_byte(view.doc(), back_block, back_byte) {
                Some(again) if again == src => {}
                other => failures.push(format!(
                    "{what}: block {} byte {local} -> src {src} -> ({back_block},{back_byte}) -> {other:?}",
                    block.block_idx
                )),
            }
        }
    }
    out
}
