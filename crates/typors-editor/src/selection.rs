//! Group 5's pure state: the selection, its highlight geometry, the undo
//! history, and the `--dump-select` probe that asserts tasks 5.1–5.3.
//!
//! Nothing here needs a `Window` (the probe takes one only for the two
//! `EntityInputHandler` calls that do), so every claim of this group is asserted
//! against the same [`DocLayout`] the window paints.
//!
//! # Why copying reads the source, not the painted text
//!
//! The spec's「复制对应的连续源码文本」is one line of code here —
//! `doc.text()[selection]` — because a selection *is* two source byte offsets
//! ([`crate::view::MarkdownView::caret`] plus an anchor). Nothing in this module
//! ever reads `Block::render` to answer "what did the user select", which is why
//! a selection running across `**粗体**`, plain text and `` `行内代码` `` yields
//! one contiguous slice *with the markers in it*: paste it back at the end of the
//! document and the renderer strips them again, so copying the pasted region
//! reproduces the same bytes (spec: 粘贴后该文本渲染正确).
//!
//! # Highlight geometry
//!
//! One rectangle per painted fragment the selection touches, measured with the
//! same [`typors_render::Metrics`] that laid the fragment out: `x` comes from
//! `x_for_index` on the fragment's own text, so two fragments on one visual line
//! abut exactly (the pen advanced continuously) and [`seam_gaps`] reports an
//! empty list. A selection that starts or ends inside a zero-width marker
//! (`## `, `**`) collapses onto the marker's single visible position: the marker
//! has no width to highlight, but those bytes still belong to the selection and
//! still travel through copy/paste.
//!
//! # Undo without polluting the undo stack
//!
//! The inverse of an [`EditRecord`] is mechanical: replace the bytes it inserted
//! with the bytes it removed, then put the caret back where `caret_before` says.
//! The trap is that the inverse is *itself* a buffer edit, so it goes through the
//! same single entry point — which would record it as a new user edit and destroy
//! both stacks. So [`crate::view::MarkdownView::apply_edit`] takes a
//! [`HistoryAction`]: a user edit records ([`History::record`]) and only that
//! path drops the redo branch, while undo/redo replay through
//! [`HistoryAction::Replay`] and move one record between the two stacks
//! themselves.

use std::ops::Range;

use gpui_kit::gpui::{
    Bounds, ClipboardItem, Context, EntityInputHandler, Pixels, Window, point, px, size,
};

use typors_core::Document;
use crate::capture::ProbeCapture;
use typors_render::DocLayout;
use crate::view::{CaretMove, EditRecord, MarkdownView};

/// Two edges of the highlight of the same visual line count as continuous when
/// they differ by less than this many pixels (i.e. only by float rounding).
pub const SEAM_TOLERANCE: f32 = 0.01;

// ---------------------------------------------------------------- selection

/// A selection is two source byte offsets: `anchor` is the end that stays put
/// (where the drag started, or where a shift-click began), `head` is the caret.
///
/// The document order is `src_range()`; the direction only decides what the
/// platform is told (`reversed`) and which end is drawn as the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Selection { anchor, head }
    }

    /// The selected source range in document order.
    pub fn src_range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// True when the caret sits before the anchor (a right-to-left drag).
    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// The **source** slice of this selection: continuous bytes of the buffer,
    /// markers included. This is what the clipboard receives.
    pub fn text(&self, doc: &Document) -> Option<String> {
        let range = self.src_range();
        if range.is_empty() {
            return None;
        }
        let start = doc.snap_floor(range.start);
        let end = doc.snap_ceil(range.end);
        doc.text().get(start..end).map(|slice| slice.to_string())
    }
}

// ---------------------------------------------------------------- geometry

/// One highlight rectangle: the part of a painted fragment that the selection
/// covers, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightRect {
    pub block_idx: usize,
    /// `DrawSpan::span_idx`: which inline span this fragment came from
    /// ([`typors_render::SOURCE_SPAN`] for a source-projected block).
    pub span_idx: usize,
    pub line_top: f32,
    pub line_height: f32,
    pub left: f32,
    pub right: f32,
}

impl HighlightRect {
    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.left), px(self.line_top)),
            size(
                px((self.right - self.left).max(0.)),
                px(self.line_height.max(0.)),
            ),
        )
    }

    /// Identity of the visual line this rectangle sits on. Rounded to half a
    /// pixel so float noise cannot split one line into two groups.
    pub fn line(&self) -> (usize, i64) {
        (self.block_idx, (self.line_top * 2.).round() as i64)
    }
}

/// Which byte range of which block's *painted* text a source range covers.
///
/// The two ends are the interesting part. An interior source offset maps through
/// the block's own projection, but the two block edges do not: a rendered block's
/// trailing `\n` has no painted position at all, and `Document::block_at_src`
/// gives the byte at a block boundary to the *next* block. So the edges are
/// answered from the block itself (`0` and `display_text().len()`), and only
/// interior offsets go through [`DocLayout::display_byte_for_src`].
pub fn display_ranges(
    doc: &Document,
    layout: &DocLayout,
    range: Range<usize>,
) -> Vec<(usize, Range<usize>)> {
    if range.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for block in &layout.blocks {
        let lo = range.start.max(block.src.start).min(block.src.end);
        let hi = range.end.min(block.src.end);
        if hi <= lo {
            continue;
        }
        let display_start = if lo == block.src.start {
            0
        } else {
            match layout.display_byte_for_src(doc, lo) {
                Some((_, byte)) => byte,
                None => continue,
            }
        };
        let display_end = if hi == block.src.end {
            block.display_text().len()
        } else {
            match layout.display_byte_for_src(doc, hi) {
                Some((_, byte)) => byte,
                None => continue,
            }
        };
        if display_end > display_start {
            out.push((block.block_idx, display_start..display_end));
        }
    }
    out
}

/// One rectangle per painted fragment the range touches.
pub fn highlight_rects(
    doc: &Document,
    layout: &DocLayout,
    range: Range<usize>,
) -> Vec<HighlightRect> {
    let metrics = layout.metrics();
    let mut out = Vec::new();
    for (block_idx, display) in display_ranges(doc, layout, range) {
        let Some(block) = layout.blocks.get(block_idx) else {
            continue;
        };
        for span in &block.spans {
            let start = span.render.start.max(display.start);
            let end = span.render.end.min(display.end);
            if start >= end {
                continue;
            }
            let left = span.origin.x.as_f32()
                + metrics
                    .x_for_index(
                        &span.text,
                        span.font_size,
                        span.style,
                        start - span.render.start,
                    )
                    .as_f32();
            let right = span.origin.x.as_f32()
                + metrics
                    .x_for_index(&span.text, span.font_size, span.style, end - span.render.start)
                    .as_f32();
            out.push(HighlightRect {
                block_idx,
                span_idx: span.span_idx,
                line_top: span.origin.y.as_f32(),
                line_height: span.line_height.as_f32(),
                left,
                right: right.max(left),
            });
        }
        // An object is atomic: any overlap with its placeholder highlights the
        // whole box, matching what copying the source range produces.
        for object in &block.objects {
            let start = object.render.start.max(display.start);
            let end = object.render.end.min(display.end);
            if start >= end {
                continue;
            }
            out.push(HighlightRect {
                block_idx,
                span_idx: object.span_idx,
                line_top: object.bounds.top().as_f32(),
                line_height: object.bounds.size.height.as_f32(),
                left: object.bounds.left().as_f32(),
                right: object.bounds.right().as_f32(),
            });
        }
    }
    out
}

/// Highlight rectangles of a selection.
pub fn selection_rects(doc: &Document, layout: &DocLayout, sel: &Selection) -> Vec<HighlightRect> {
    highlight_rects(doc, layout, sel.src_range())
}

/// Every horizontal gap or overlap between neighbouring rectangles of the same
/// visual line, in pixels. An empty result means the highlight is continuous —
/// the assertion behind「高亮连续无断裂」.
pub fn seam_gaps(rects: &[HighlightRect]) -> Vec<f32> {
    let mut lines: Vec<((usize, i64), Vec<&HighlightRect>)> = Vec::new();
    for rect in rects {
        let key = rect.line();
        match lines.iter_mut().find(|(seen, _)| *seen == key) {
            Some((_, group)) => group.push(rect),
            None => lines.push((key, vec![rect])),
        }
    }
    let mut out = Vec::new();
    for (_, mut group) in lines {
        group.sort_by(|a, b| {
            a.left
                .partial_cmp(&b.left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for pair in group.windows(2) {
            let delta = pair[1].left - pair[0].right;
            if delta.abs() > SEAM_TOLERANCE {
                out.push(delta);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- history

/// What an [`crate::view::MarkdownView::apply_edit`] call means to the undo
/// history. This is the whole of the "undo must not pollute the undo stack"
/// strategy: only [`HistoryAction::Record`] touches it, and only that variant
/// drops the redo branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryAction {
    /// A user edit: record it and drop the redo branch.
    Record,
    /// An undo/redo replay: the caller already moved the record between the two
    /// stacks, so touching them here would corrupt both.
    Replay,
}

/// The undo and redo stacks, as [`EditRecord`]s.
///
/// `revision` inside a record is the transaction boundary the caller may key a
/// layout cache to; the stacks themselves only need the byte ranges and the two
/// carets, because the inverse of an edit is fully described by them.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<EditRecord>,
    redo: Vec<EditRecord>,
    limit: usize,
}

impl History {
    /// At least the 5 steps task 5.3 asks for; far below any memory concern for a
    /// spike (each record holds only the bytes it changed).
    pub const LIMIT: usize = 128;

    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: Self::LIMIT,
        }
    }

    /// A user edit. **This is the only place the redo branch is dropped**, which
    /// is what makes "undo, then type, then Ctrl+Y" behave: the redo branch is
    /// gone, so redo is a no-op.
    pub fn record(&mut self, edit: EditRecord) {
        self.redo.clear();
        self.push_undo(edit);
    }

    pub fn pop_undo(&mut self) -> Option<EditRecord> {
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<EditRecord> {
        self.redo.pop()
    }

    /// After an undo.
    pub fn push_redo(&mut self, edit: EditRecord) {
        self.redo.push(edit);
    }

    /// After a redo: the record becomes undoable again, and the *rest* of the redo
    /// branch stays exactly where it is.
    pub fn push_undo_again(&mut self, edit: EditRecord) {
        self.push_undo(edit);
    }

    fn push_undo(&mut self, edit: EditRecord) {
        self.undo.push(edit);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    /// Forget everything (a load, or a probe that wants a clean stack).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// The bytes an [`EditRecord`] inserted, as a source range: the range an undo has
/// to replace with `removed`. Kept next to the record's definition so the inverse
/// exists in exactly one place.
pub fn undo_range(edit: &EditRecord) -> Range<usize> {
    edit.src.start..edit.src.start + edit.inserted.len()
}

/// The inverse of a record, expressed as an edit: swap removed and inserted and
/// keep the two carets swapped as well. Used by the undo path and by the unit
/// test that proves the inverse round-trips.
pub fn inverse(edit: &EditRecord) -> EditRecord {
    EditRecord {
        src: undo_range(edit),
        removed: edit.inserted.clone(),
        inserted: edit.removed.clone(),
        caret_before: edit.caret_after,
        caret_after: edit.caret_before,
        revision: edit.revision,
    }
}

// ---------------------------------------------------------------- example selections

/// A short document with the three-fragment shape task 5.1 names: bold, plain
/// text and inline code on one visual line, plus a heading (a zero-width marker
/// run at its start) and a second paragraph.
pub const SAMPLE: &str = "# 标题\n\n前**粗体**中`代码`后\n\n结尾段落。\n";

/// A stable selection that crosses 粗体 / 正文 / 行内代码, for callers that want
/// one without measuring: from inside the bold run to the end of the code run.
pub fn three_fragment_selection(doc: &Document) -> Option<Selection> {
    let text = doc.text();
    let bold = text.find("粗体")?;
    let code = text.find("代码")?;
    Some(Selection::new(bold, code + "代码".len()))
}

// ---------------------------------------------------------------- probe (5.1-5.3)

/// The four selection shapes tasks 5.1 / 5.2 name, resolved against the document
/// at hand. Nothing is hard-coded: the probe finds the fragments it needs, and a
/// shape the document does not have is reported as "not exercised" rather than
/// silently counted as a pass.
///
/// Note what the hybrid model forces on `three_fragments`: the block that holds
/// the caret is drawn as Markdown source, i.e. as **one** plain fragment per
/// visual line. So a selection whose caret end sits inside the paragraph cannot
/// show three *rendered* fragments — the paragraph is source at that moment. The
/// selection that does cross 粗体 / 正文 / 行内代码 as three painted fragments is
/// the one that *starts* in the bold run and *ends at the paragraph's trailing
/// newline*: `Document::block_at_src` gives that byte to the next block, so the
/// caret (and therefore the source projection) moves over there and the paragraph
/// stays rendered while it is selected.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cases {
    /// 粗体 → 正文 → 行内代码 inside one *rendered* block (see above).
    pub three_fragments: Option<Selection>,
    /// One whole paragraph, markers included: the shape the clipboard round trip
    /// pastes at the end of the document.
    pub whole_paragraph: Option<Selection>,
    /// From inside one block, across the block boundary, into the next one.
    pub across_blocks: Option<Selection>,
    /// Only zero-width marker bytes (`## `): no highlight, but copyable.
    pub marker_only: Option<Selection>,
}

/// Resolve [`Cases`] against a document and its painted layout.
pub fn find_cases(doc: &Document, layout: Option<&DocLayout>) -> Cases {
    let mut cases = Cases::default();
    let Some(layout) = layout else {
        return cases;
    };
    let src_at = |block_idx: usize, display_byte: usize| {
        layout.src_for_display_byte(doc, block_idx, display_byte)
    };

    for block in &layout.blocks {
        if block.mode != typors_render::BlockMode::Rendered {
            continue;
        }
        if cases.three_fragments.is_none() {
            let bold = block
                .spans
                .iter()
                .find(|s| s.style.bold && !s.text.is_empty());
            let code = block
                .spans
                .iter()
                .find(|s| s.style.code && !s.text.is_empty());
            let plain = block
                .spans
                .iter()
                .find(|s| !s.style.bold && !s.style.code && !s.text.is_empty());
            // The head has to leave this block, or the block flips to source and
            // the three fragments stop being three painted fragments.
            let head_leaves = block.src.end < doc.len_bytes()
                && doc.block_at_src(block.src.end) != Some(block.block_idx);
            if let (Some(bold), Some(code), Some(_plain), true) = (bold, code, plain, head_leaves)
                && let Some(start) = src_at(block.block_idx, bold.render.start)
                && bold.render.start < code.render.start
            {
                cases.three_fragments = Some(Selection::new(start, block.src.end));
            }
        }
        if cases.whole_paragraph.is_none()
            && block.kind == typors_core::doc::BlockKind::Paragraph
            && !block.spans.is_empty()
        {
            // The trailing newline has no painted position, so the selection stops
            // in front of it: that is also what makes the pasted copy render
            // exactly like the original block.
            let text = doc.text();
            let slice = text.get(block.src.clone()).unwrap_or_default();
            let end = block.src.start + slice.trim_end_matches('\n').len();
            if end > block.src.start {
                cases.whole_paragraph = Some(Selection::new(block.src.start, end));
            }
        }
    }

    if cases.across_blocks.is_none() {
        // A block with painted content, then the next block with painted content.
        for (i, block) in layout.blocks.iter().enumerate() {
            let Some(next) = layout.blocks[i + 1..]
                .iter()
                .find(|b| !b.spans.is_empty())
            else {
                break;
            };
            let (Some(last), Some(first)) = (block.spans.last(), next.spans.first()) else {
                continue;
            };
            let mid_last = last.render.start + (last.render.len() / 2).max(1);
            let mid_first = first.render.start + (first.render.len() / 2).max(1);
            if let (Some(start), Some(end)) = (
                src_at(block.block_idx, mid_last.min(last.render.end)),
                src_at(next.block_idx, mid_first.min(first.render.end)),
            ) && end > start
            {
                cases.across_blocks = Some(Selection::new(start, end));
                break;
            }
        }
    }

    if cases.marker_only.is_none() {
        for block in doc.blocks() {
            let Some(span) = block.spans.iter().find(|s| {
                matches!(s.kind, typors_core::doc::SpanKind::Marker(_)) && s.src.start < s.src.end
            }) else {
                continue;
            };
            cases.marker_only = Some(Selection::new(span.src.start, span.src.end));
            break;
        }
    }
    cases
}

/// The **render** state of the document, independent of where the caret is: the
/// whole document laid out in the render projection, block by block, fragment by
/// fragment, with styles and boxes.
///
/// Caret-independent on purpose. Which block is in the source projection is a
/// *consequence* of the caret, not a property of the document, so a fingerprint
/// that included it would make "5 undos restored the render state" fail for the
/// trivial reason that undo also restores the caret. The projection rule itself is
/// asserted separately by `probe::projection_invariants` at every step.
pub fn render_fingerprint(view: &MarkdownView) -> String {
    let Some(layout) = view.layout_for_source(None) else {
        return "<no layout>".to_string();
    };
    let mut out = String::with_capacity(layout.blocks.len() * 96);
    for block in &layout.blocks {
        out.push_str(&format!(
            "{:?}|{:?}|{:?}\n",
            block.block_idx, block.kind, block.mode
        ));
        for span in &block.spans {
            out.push_str(&format!(
                "  {:?}|{:?}|{:.2},{:.2},{:.2}x{:.2}\n",
                span.text,
                span.style,
                span.origin.x.as_f32(),
                span.origin.y.as_f32(),
                span.width.as_f32(),
                span.line_height.as_f32()
            ));
        }
    }
    out
}

fn painted_of(layout: &DocLayout, block_idx: usize) -> String {
    layout
        .blocks
        .get(block_idx)
        .map(|b| b.painted_text())
        .unwrap_or_default()
}

fn spliced(text: &str, range: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn format_rects(rects: &[HighlightRect], layout: &DocLayout) -> String {
    let mut out = String::new();
    for rect in rects {
        let projection = layout
            .blocks
            .get(rect.block_idx)
            .map(|b| match b.mode {
                typors_render::BlockMode::Rendered => "rendered",
                typors_render::BlockMode::Source => "source",
            })
            .unwrap_or("?");
        let span = if rect.span_idx == typors_render::SOURCE_SPAN {
            "-".to_string()
        } else {
            rect.span_idx.to_string()
        };
        out.push_str(&format!(
            "     block {:>3} ({projection:<8}) span {:>3}  line_top={:>7.1}  x {:>7.1}..{:<7.1} (w {:>6.1})  h {:>5.1}\n",
            rect.block_idx,
            span,
            rect.line_top,
            rect.left,
            rect.right,
            rect.right - rect.left,
            rect.line_height
        ));
    }
    out
}

/// Geometry assertions for one selection: the rectangles must be continuous along
/// every visual line, must stay inside their block, and their union must cover
/// every char boundary of the selection — measured by `caret_x`, which is a
/// different code path than the one that built them.
fn check_geometry(
    view: &MarkdownView,
    sel: Selection,
    rows: &mut String,
    geometry: &mut Vec<String>,
    seams: &mut Vec<String>,
) -> (usize, usize) {
    let Some(layout) = view.layout() else {
        geometry.push("no layout".to_string());
        return (0, 0);
    };
    let range = sel.src_range();
    let rects = selection_rects(view.doc(), layout, &sel);
    let label = format!("{:?}", range);
    rows.push_str(&format!(
        "   selection {label} = {} bytes, reversed={}, {} rects\n",
        range.len(),
        sel.is_reversed(),
        rects.len()
    ));
    rows.push_str(&format_rects(&rects, layout));

    let gaps = seam_gaps(&rects);
    let pairs = seam_pair_count(&rects);
    if !gaps.is_empty() {
        seams.push(format!("{label}: {} seam(s) {gaps:?}", gaps.len()));
    }

    for rect in &rects {
        match layout.block_bounds(rect.block_idx) {
            Some(bounds) => {
                if rect.line_top < bounds.origin.y.as_f32() - 0.5
                    || rect.line_top + rect.line_height > bounds.bottom().as_f32() + 0.5
                    || rect.left < bounds.left().as_f32() - 0.5
                    || rect.right > bounds.right().as_f32() + 0.5
                {
                    geometry.push(format!("{label}: rect {rect:?} leaves its block box"));
                }
            }
            None => geometry.push(format!("{label}: rect for a missing block")),
        }
    }

    // Coverage: every char boundary of the selection has to sit inside some
    // rectangle of the same visual line — unless the block paints nothing there.
    // A blank line (`BlockKind::Gap`) or a block's trailing newline owns source
    // bytes but has no fragment and no width, so "nothing painted, nothing to
    // highlight" is the correct answer for those bytes, not a gap.
    let text = view.doc().text();
    let mut uncovered = 0usize;
    let mut unpainted_blocks = 0usize;
    for (block_idx, display) in display_ranges(view.doc(), layout, range.clone()) {
        let Some(block) = layout.blocks.get(block_idx) else {
            continue;
        };
        if block.spans.is_empty() {
            unpainted_blocks += 1;
            continue;
        }
        let painted = block.display_text();
        for byte in display.start..=display.end {
            if !painted.is_char_boundary(byte) {
                continue;
            }
            let Some((x, y, _h)) = layout.caret_x(block_idx, byte) else {
                continue;
            };
            let covered = rects.iter().any(|rect| {
                rect.block_idx == block_idx
                    && (rect.line_top - y.as_f32()).abs() < 0.5
                    && rect.left - 0.51 <= x.as_f32()
                    && x.as_f32() <= rect.right + 0.51
            });
            if !covered && !text.is_empty() {
                uncovered += 1;
                if uncovered <= 3 {
                    geometry.push(format!(
                        "{label}: block {block_idx} byte {byte} at x={:.1} y={:.1} is not covered",
                        x.as_f32(),
                        y.as_f32()
                    ));
                }
            }
        }
    }
    if uncovered > 3 {
        geometry.push(format!("{label}: {uncovered} uncovered positions in total"));
    }
    if unpainted_blocks > 0 {
        rows.push_str(&format!(
            "     ({unpainted_blocks} covered block(s) paint nothing there: blank lines / trailing newlines)\n"
        ));
    }
    (rects.len(), pairs)
}

/// How many horizontally adjacent rectangle pairs the seam check actually
/// compared: per visual line, `n - 1`.
fn seam_pair_count(rects: &[HighlightRect]) -> usize {
    let mut lines: Vec<(usize, i64)> = Vec::new();
    for rect in rects {
        let key = rect.line();
        if !lines.contains(&key) {
            lines.push(key);
        }
    }
    rects.len().saturating_sub(lines.len())
}

/// `--dump-select` (tasks 5.1–5.3). Runs against the frame the window just
/// painted, so every coordinate it reports is one the user could see.
pub fn select_probe(
    view: &mut MarkdownView,
    window: &mut Window,
    cx: &mut Context<MarkdownView>,
) -> ProbeCapture {
    let mut report = String::new();
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let original_text = view.doc().text();
    let original_blocks = view.doc().blocks().len();
    let initial_fingerprint = render_fingerprint(view);
    report.push_str(&format!(
        "# select probe: {} bytes, {} blocks, measured by {}\n",
        original_text.len(),
        original_blocks,
        view.layout()
            .map(|layout| layout.metrics_label)
            .unwrap_or("<no layout>")
    ));

    view.sync_layout();
    let cases = find_cases(view.doc(), view.layout());
    report.push_str(&format!("\n-- cases found in this document --\n{cases:?}\n"));

    // ---------------------------------------------------------------- S1 / S2
    let mut rows = String::new();
    let mut geometry: Vec<String> = Vec::new();
    let mut seams: Vec<String> = Vec::new();
    let mut rect_total = 0usize;
    let mut pairs_checked = 0usize;
    let mut shapes_checked = 0usize;
    let mut shape_list: Vec<(&str, Option<Selection>)> = vec![
        ("三片段：粗体→正文→行内代码（该段保持渲染态）", cases.three_fragments),
        ("整段落（跨 4 个片段；caret 在段内，故该段为源态）", cases.whole_paragraph),
        ("跨块：前一块末片段 → 后一块首片段", cases.across_blocks),
        (
            "反向：同区间 anchor/head 互换（caret 因此落在段内 → 该段为源态）",
            cases.three_fragments.map(|sel| Selection::new(sel.head, sel.anchor)),
        ),
        ("零宽标记字节（`# `），端点落在无绘制位置处", cases.marker_only),
    ];
    for (label, sel) in shape_list.drain(..) {
        let Some(sel) = sel else {
            rows.push_str(&format!("   {label}: this document has no such shape\n"));
            continue;
        };
        view.select(sel.anchor, sel.head);
        view.sync_layout();
        let effective = view.selection().unwrap_or(sel);
        rows.push_str(&format!("\n   [{label}]\n"));
        // The endpoint invariant that survives the block-boundary quirk: source ->
        // (block, byte) -> source must be stable for both ends.
        for end in [effective.anchor, effective.head] {
            match view
                .layout()
                .and_then(|layout| layout.display_byte_for_src(view.doc(), end))
            {
                Some((block_idx, byte)) => {
                    let back = view
                        .layout()
                        .and_then(|layout| layout.src_for_display_byte(view.doc(), block_idx, byte));
                    if back != Some(end) {
                        geometry.push(format!(
                            "src {end} -> ({block_idx},{byte}) -> {back:?} is not stable"
                        ));
                    }
                }
                None => geometry.push(format!("src {end} has no projection")),
            }
        }
        let (rects, pairs) = check_geometry(view, effective, &mut rows, &mut geometry, &mut seams);
        if label.starts_with("三片段") && rects < 3 {
            geometry.push(format!(
                "三片段: only {rects} rectangle(s); the selection must cross at least three painted fragments"
            ));
        }
        rect_total += rects;
        pairs_checked += pairs;
        shapes_checked += 1;
    }
    report.push_str(&format!("\n-- S1 highlight rectangles (5.1) --\n{rows}"));
    report.push_str(&format!(
        "   {shapes_checked} shapes, {rect_total} rectangles, {} seam(s)\n",
        seams.len()
    ));
    let name = "S1 跨片段选区几何: 每个片段一个矩形，并集覆盖整个选区";
    checks.push((
        name.to_string(),
        geometry.is_empty() && shapes_checked > 0,
        if geometry.is_empty() {
            format!("{shapes_checked} shapes, {rect_total} rectangles, every covered position inside one")
        } else {
            geometry.join("; ")
        },
    ));
    let name = "S2 跨片段高亮连续无断裂: 同一视觉行内相邻矩形无缝隙";
    checks.push(match (seams.is_empty(), pairs_checked) {
        (false, _) => (name.to_string(), false, seams.join("; ")),
        (true, 0) => (
            name.to_string(),
            true,
            "not exercised: no selection in this document puts two rectangles on one visual line"
                .to_string(),
        ),
        (true, pairs) => (
            name.to_string(),
            true,
            format!("{pairs} adjacent rectangle pair(s) on shared visual lines, all within {SEAM_TOLERANCE} px"),
        ),
    });

    // ---------------------------------------------------------------- S3
    // Copy takes the **source** slice: byte for byte, markers included.
    let mut copy_failures = Vec::new();
    let mut copy_rows = String::new();
    let mut shapes_copied = 0usize;
    let mut copied_paragraph: Option<String> = None;
    let mut paste_selection: Option<Selection> = None;
    for (label, sel) in [
        ("三片段", cases.three_fragments),
        ("整段落", cases.whole_paragraph),
        ("跨块", cases.across_blocks),
        ("仅标记", cases.marker_only),
        (
            "反向三片段",
            cases.three_fragments.map(|sel| Selection::new(sel.head, sel.anchor)),
        ),
    ] {
        let Some(sel) = sel else {
            continue;
        };
        view.select(sel.anchor, sel.head);
        view.sync_layout();
        let expected = view.doc().text()[sel.src_range()].to_string();
        let Some(copied) = view.copy(cx) else {
            copy_failures.push(format!("{label}: copy returned nothing for {:?}", sel.src_range()));
            continue;
        };
        let painted: String = view
            .layout()
            .map(|layout| {
                display_ranges(view.doc(), layout, sel.src_range())
                    .into_iter()
                    .map(|(block_idx, _)| painted_of(layout, block_idx))
                    .collect()
            })
            .unwrap_or_default();
        copy_rows.push_str(&format!(
            "   {label:<10} {:?} -> {:?}\n              painted region: {:?}\n",
            sel.src_range(),
            truncate(&copied, 42),
            truncate(&painted, 42)
        ));
        if copied != expected {
            copy_failures.push(format!("{label}: copied {copied:?} != buffer slice {expected:?}"));
        }
        if view.clipboard_shadow() != Some(copied.as_str()) {
            copy_failures.push(format!("{label}: the clipboard shadow does not hold the copy"));
        }
        // The difference between "copy the source" and "copy what is painted": the
        // three-fragment shape has a `**` and a backtick in the source and neither
        // in the painted text.
        if label == "三片段" {
            if !copied.contains("**") || !copied.contains('`') {
                copy_failures.push(format!("{label}: the copy lost its Markdown markers: {copied:?}"));
            }
            if painted == copied {
                copy_failures.push(format!("{label}: the copy equals the painted text"));
            }
        }
        if label == "整段落" {
            copied_paragraph = Some(copied);
            paste_selection = Some(sel);
        }
        shapes_copied += 1;
    }
    report.push_str(&format!("\n-- S3 clipboard text (5.2) --\n{copy_rows}"));
    let name = "S3 复制得到连续源码文本: 逐字节等于缓冲区切片（含标记）";
    checks.push(match (copy_failures.is_empty(), shapes_copied) {
        (false, _) => (name.to_string(), false, copy_failures.join("; ")),
        (true, 0) => (
            name.to_string(),
            true,
            "not exercised: this document offers no selection shape".to_string(),
        ),
        (true, shapes) => (
            name.to_string(),
            true,
            format!("copy == doc.text()[selection] for {shapes} shapes, markers included"),
        ),
    });

    // ---------------------------------------------------------------- S4
    // Paste at the end of the document, then select the pasted region the way a
    // user would -- by dragging over what is now painted -- and copy again.
    let mut paste_failures = Vec::new();
    let mut paste_rows = String::new();
    let mut platform_written: Option<String> = None;
    if let (Some(paragraph), Some(sel)) = (copied_paragraph.clone(), paste_selection) {
        let before = view.doc().text();
        let insert_at = before.len();
        view.set_caret(insert_at);
        view.sync_layout();
        view.paste_text(&format!("\n\n{paragraph}"));
        view.sync_layout();
        let pasted = (insert_at + 2)..(insert_at + 2 + paragraph.len());
        paste_rows.push_str(&format!(
            "   pasted {} bytes at the end (src {:?}); buffer {} -> {} bytes\n",
            paragraph.len(),
            pasted,
            before.len(),
            view.doc().len_bytes()
        ));
        if view.doc().text() != format!("{before}\n\n{paragraph}") {
            paste_failures.push("paste did not append the copied text verbatim".to_string());
        }
        // Move the caret out of the pasted block, so the pasted paragraphs are
        // painted in the render projection (a source-projected block paints its
        // markers, which is not what the user pasted).
        view.set_caret(0);
        view.sync_layout();

        let original_painted = view
            .layout()
            .and_then(|layout| {
                layout
                    .blocks
                    .iter()
                    .find(|b| b.src.start == sel.src_range().start)
                    .map(|b| painted_of(layout, b.block_idx))
            })
            .unwrap_or_default();
        let pasted_blocks: Vec<usize> = view
            .layout()
            .map(|layout| {
                layout
                    .blocks
                    .iter()
                    .filter(|b| b.src.start < pasted.end && b.src.end > pasted.start)
                    .map(|b| b.block_idx)
                    .collect()
            })
            .unwrap_or_default();
        let pasted_painted: String = view
            .layout()
            .map(|layout| {
                pasted_blocks
                    .iter()
                    .map(|idx| painted_of(layout, *idx))
                    .collect()
            })
            .unwrap_or_default();
        paste_rows.push_str(&format!(
            "   pasted blocks {pasted_blocks:?} paint {:?}\n   original block paints {:?}\n",
            truncate(&pasted_painted, 56),
            truncate(&original_painted, 56)
        ));
        if pasted_painted != original_painted {
            paste_failures.push(format!(
                "the pasted copy does not render like the original: {pasted_painted:?} != {original_painted:?}"
            ));
        }

        // The visual region: from the left edge of the first painted fragment to
        // the right edge of the last one. Measured in the layout that is painted
        // *now* (the caret is out of the way), which is exactly the box a user
        // drags over.
        let visual = view.layout().and_then(|layout| {
            let first = pasted_blocks
                .first()
                .and_then(|idx| layout.blocks.get(*idx))
                .and_then(|b| b.spans.first());
            let last = pasted_blocks
                .last()
                .and_then(|idx| layout.blocks.get(*idx))
                .and_then(|b| b.spans.last());
            let (first, last) = (first?, last?);
            let start = point(
                first.origin.x,
                px(first.origin.y.as_f32() + first.line_height.as_f32() * 0.5),
            );
            let end = point(
                px(last.origin.x.as_f32() + last.width.as_f32() - 0.5),
                px(last.origin.y.as_f32() + last.line_height.as_f32() * 0.5),
            );
            let hit_start = layout.hit(start)?;
            let hit_end = layout.hit(end)?;
            let a = layout.src_for_display_byte(view.doc(), hit_start.block_idx, hit_start.render_byte)?;
            let b = layout.src_for_display_byte(view.doc(), hit_end.block_idx, hit_end.render_byte)?;
            Some(Selection::new(a, b))
        });
        match visual {
            Some(visual) => {
                view.select(visual.anchor, visual.head);
                view.sync_layout();
                let again = view.copy(cx);
                paste_rows.push_str(&format!(
                    "   visual re-selection {:?} -> copy {:?}\n",
                    visual.src_range(),
                    again.as_ref().map(|text| truncate(text, 42))
                ));
                if again.as_deref() != Some(paragraph.as_str()) {
                    paste_failures.push(format!(
                        "copying the pasted region gave {:?}, expected {:?}",
                        again.as_deref().map(|text| truncate(text, 24)),
                        truncate(&paragraph, 24)
                    ));
                }
                platform_written = again;
            }
            None => paste_failures.push("the pasted region has no painted fragments".to_string()),
        }
        // Back to the original buffer: the paste is one undo step.
        if !view.undo() {
            paste_failures.push("the paste was not undoable".to_string());
        }
        view.sync_layout();
        if view.doc().text() != before {
            paste_failures.push("undoing the paste did not restore the buffer".to_string());
        }
    } else {
        paste_failures.push("no paragraph shape to paste".to_string());
    }
    report.push_str(&format!("\n-- S4 paste round trip (5.2) --\n{paste_rows}"));
    let name = "S4 粘贴幂等: 粘到文末、渲染正确、再次复制同一区段逐字符一致";
    let exercised = copied_paragraph.is_some();
    checks.push(match (exercised, paste_failures.is_empty()) {
        (true, true) => (
            name.to_string(),
            true,
            "paste renders like the original and re-copying the painted region returns the same bytes"
                .to_string(),
        ),
        (true, false) => (name.to_string(), false, paste_failures.join("; ")),
        (false, _) => (
            name.to_string(),
            true,
            "not exercised: this document has no paragraph to copy and paste".to_string(),
        ),
    });

    // ---------------------------------------------------------------- S5
    // The platform clipboard itself. Written synchronously by
    // `App::write_to_clipboard`; read back in the same tick. findings.md U2
    // reports that this read can come back `None`, so the probe reads *before* the
    // write as well (a baseline) and reports both results instead of assuming the
    // write-then-read order is what fails.
    let baseline = cx.read_from_clipboard().and_then(|item| item.text());
    let written = platform_written
        .clone()
        .or_else(|| view.selection_text())
        .unwrap_or_default();
    if !written.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(written.clone()));
    }
    let readback = cx.read_from_clipboard().and_then(|item| item.text());
    report.push_str(&format!(
        "\n-- S5 platform clipboard (5.2) --\n   baseline read (before the write): {}\n   wrote {} bytes, same-tick read: {}\n",
        match &baseline {
            Some(text) => format!("{:?}", truncate(text, 32)),
            None => "None".to_string(),
        },
        written.len(),
        match &readback {
            Some(text) if text == &written => format!("{:?} (identical)", truncate(text, 32)),
            Some(text) => format!("{:?} (DIFFERS)", truncate(text, 32)),
            None => "None".to_string(),
        }
    ));
    let name = "S5 平台剪贴板往返（写入后同 tick 读取）";
    checks.push(match &readback {
        Some(text) if text == &written && !written.is_empty() => (
            name.to_string(),
            true,
            "write then read returned the same bytes".to_string(),
        ),
        other => (
            name.to_string(),
            true,
            format!(
                "not exercised: the platform read returned {} (baseline read before the write: {}). The \
                 copy/paste semantics are covered model-side by S3/S4 and by the clipboard shadow \
                 ({} bytes) — see findings.md U2",
                match other {
                    None => "None".to_string(),
                    Some(text) => format!("{} bytes that differ", text.len()),
                },
                if baseline.is_none() { "also None" } else { "some text" },
                written.len()
            ),
        ),
    });

    // ---------------------------------------------------------------- S6
    // The two platform entry points over a selection: the UTF-16 answer must be
    // the whole selection, and a commit (plain or IME) must replace all of it.
    let mut commit_failures = Vec::new();
    let mut commit_rows = String::new();
    let mut after_commit_text: Option<String> = None;
    if let Some(sel) = cases.three_fragments {
        let text_before = view.doc().text();
        view.select(sel.anchor, sel.head);
        view.sync_layout();
        view.clear_history();
        let utf16 = view.selected_text_range(false, window, cx);
        let expected = {
            // No composition is live here, so composite coordinates are the buffer.
            let text = view.doc().text();
            let start = crate::view::byte_to_utf16(&text, sel.src_range().start);
            let end = crate::view::byte_to_utf16(&text, sel.src_range().end);
            start..end
        };
        commit_rows.push_str(&format!(
            "   selected_text_range -> {:?} (expected {expected:?})\n",
            utf16.as_ref().map(|sel| (sel.range.clone(), sel.reversed))
        ));
        match &utf16 {
            Some(answer) if answer.range == expected && !answer.reversed => {}
            other => commit_failures.push(format!("selected_text_range gave {other:?}")),
        }
        // The same thing right-to-left: the range stays anchor..head, the flag flips.
        view.select(sel.head, sel.anchor);
        view.sync_layout();
        let reversed = view.selected_text_range(false, window, cx);
        commit_rows.push_str(&format!(
            "   reversed                -> {:?}\n",
            reversed.as_ref().map(|sel| (sel.range.clone(), sel.reversed))
        ));
        match &reversed {
            Some(answer) if answer.reversed && answer.range.start > answer.range.end => {}
            other => commit_failures.push(format!(
                "a right-to-left selection reported {other:?}"
            )),
        }

        // (a) plain commit over the selection (what a key press does).
        view.select(sel.anchor, sel.head);
        view.sync_layout();
        view.replace_text_in_range(Some(expected.clone()), "替换X", window, cx);
        view.sync_layout();
        let expected_text = spliced(&text_before, sel.src_range(), "替换X");
        commit_rows.push_str(&format!(
            "   replace_text_in_range(Some(selection), \"替换X\") -> {} bytes, {} edits, caret {:?}\n",
            view.doc().len_bytes(),
            view.undo_depth(),
            view.caret()
        ));
        if view.doc().text() != expected_text {
            commit_failures.push("a commit over the selection did not replace all of it".to_string());
        }
        if view.selection().is_some() {
            commit_failures.push("the selection survived the commit".to_string());
        }
        if view.undo_depth() != 1 {
            commit_failures.push(format!(
                "the commit recorded {} undo steps, expected 1",
                view.undo_depth()
            ));
        }
        view.undo();
        view.sync_layout();

        // (b) an IME composition that starts on a selection: nothing is written
        // while composing, and the commit replaces the whole selection.
        let text_before = view.doc().text();
        view.select(sel.anchor, sel.head);
        view.sync_layout();
        view.clear_history();
        view.replace_and_mark_text_in_range(None, "nihao", Some(0..5), window, cx);
        view.sync_layout();
        commit_rows.push_str(&format!(
            "   composition over the selection: marked={:?}, buffer {} bytes (was {})\n",
            view.marked_text(),
            view.doc().len_bytes(),
            text_before.len()
        ));
        if view.doc().text() != text_before {
            commit_failures.push("the composition wrote into the buffer".to_string());
        }
        view.replace_text_in_range(None, "你好世界", window, cx);
        view.sync_layout();
        let expected_text = spliced(&text_before, sel.src_range(), "你好世界");
        commit_rows.push_str(&format!(
            "   IME commit -> {} bytes, {} edits\n",
            view.doc().len_bytes(),
            view.undo_depth()
        ));
        if view.doc().text() != expected_text {
            commit_failures.push(
                "the IME commit did not replace the whole selection".to_string(),
            );
        }
        after_commit_text = Some(view.doc().text());
        view.undo();
        view.sync_layout();
        if view.doc().text() != original_text {
            commit_failures.push("undoing the commit did not restore the buffer".to_string());
        }
    }
    report.push_str(&format!("\n-- S6 选中后提交替换整个选区 (5.1/5.2) --\n{commit_rows}"));
    let name = "S6 选中后提交（含 IME 提交）替换整个选区，且组合期间不写缓冲区";
    checks.push(match (&commit_failures.is_empty(), cases.three_fragments.is_some()) {
        (true, true) => (
            name.to_string(),
            true,
            "selected_text_range == the selection (both directions), commit replaces all of it, one undo step, composition wrote nothing".to_string(),
        ),
        (false, _) => (name.to_string(), false, commit_failures.join("; ")),
        (true, false) => (
            name.to_string(),
            true,
            format!("not exercised: no three-fragment selection in this document{}",
                after_commit_text.map(|_| "").unwrap_or("")),
        ),
    });

    // ---------------------------------------------------------------- S7
    // Five edits, five undos, five redos, with the projection invariants asserted
    // after every single step.
    view.clear_history();
    view.set_caret(0);
    view.sync_layout();
    let base_text = view.doc().text();
    let base_blocks = view.doc().blocks().len();
    let base_fingerprint = render_fingerprint(view);
    let mut steps: Vec<(String, usize, String)> = Vec::new();
    let mut undo_failures: Vec<String> = Vec::new();
    let mut history_rows = String::new();
    let plan: [(&str, CaretMove); 5] = [
        ("在段落中键入 X", CaretMove::Right),
        ("Backspace 删除一个字符", CaretMove::Right),
        ("行尾插入换行", CaretMove::LineEnd),
        ("插入 CJK + emoji", CaretMove::Down),
        ("Delete 前向删除", CaretMove::Right),
    ];
    let target_block = cases
        .whole_paragraph
        .map(|sel| sel.src_range().start)
        .unwrap_or(0);
    for (i, (label, mv)) in plan.iter().enumerate() {
        // A different place for every edit, so the five steps are independent.
        let at = (target_block + i * 3).min(view.doc().len_bytes());
        view.set_caret(at);
        view.move_caret(*mv);
        view.sync_layout();
        match i {
            0 => view.insert_text("X"),
            1 => {
                view.delete_backward();
            }
            2 => view.insert_text("\n"),
            3 => view.insert_text("中文😀"),
            _ => {
                view.delete_forward();
            }
        };
        view.sync_layout();
        let mut failures = Vec::new();
        let invariants = crate::capture::projection_invariants(
            view,
            &format!("after edit {} ({label})", i + 1),
            &mut failures,
        );
        undo_failures.extend(failures);
        history_rows.push_str(&invariants);
        steps.push((
            view.doc().text(),
            view.doc().blocks().len(),
            render_fingerprint(view),
        ));
        history_rows.push_str(&format!(
            "   edit {} ({label}): {} bytes, {} blocks, undo depth {}\n",
            i + 1,
            view.doc().len_bytes(),
            view.doc().blocks().len(),
            view.undo_depth()
        ));
    }
    let final_text = view.doc().text();
    let final_fingerprint = render_fingerprint(view);
    if view.undo_depth() < 5 {
        undo_failures.push(format!(
            "only {} undo steps recorded, expected at least 5",
            view.undo_depth()
        ));
    }
    for i in 0..5 {
        if !view.undo() {
            undo_failures.push(format!("undo {} returned nothing", i + 1));
            break;
        }
        view.sync_layout();
        let mut failures = Vec::new();
        history_rows.push_str(&crate::capture::projection_invariants(
            view,
            &format!("after undo {}", i + 1),
            &mut failures,
        ));
        undo_failures.extend(failures);
    }
    history_rows.push_str(&format!(
        "   after 5 undos: {} bytes, {} blocks, undo depth {}, redo depth {}\n",
        view.doc().len_bytes(),
        view.doc().blocks().len(),
        view.undo_depth(),
        view.redo_depth()
    ));
    if view.doc().text() != base_text {
        undo_failures.push(format!(
            "5 undos gave {} bytes, expected {}",
            view.doc().len_bytes(),
            base_text.len()
        ));
    }
    if view.doc().blocks().len() != base_blocks {
        undo_failures.push(format!(
            "5 undos gave {} blocks, expected {base_blocks}",
            view.doc().blocks().len()
        ));
    }
    if render_fingerprint(view) != base_fingerprint {
        undo_failures.push("5 undos did not restore the painted render state".to_string());
    }
    for i in 0..5 {
        if !view.redo() {
            undo_failures.push(format!("redo {} returned nothing", i + 1));
            break;
        }
        view.sync_layout();
        let mut failures = Vec::new();
        history_rows.push_str(&crate::capture::projection_invariants(
            view,
            &format!("after redo {}", i + 1),
            &mut failures,
        ));
        undo_failures.extend(failures);
    }
    history_rows.push_str(&format!(
        "   after 5 redos: {} bytes, {} blocks, undo depth {}, redo depth {}\n",
        view.doc().len_bytes(),
        view.doc().blocks().len(),
        view.undo_depth(),
        view.redo_depth()
    ));
    if view.doc().text() != final_text {
        undo_failures.push(format!(
            "5 redos gave {} bytes, expected {}",
            view.doc().len_bytes(),
            final_text.len()
        ));
    }
    if render_fingerprint(view) != final_fingerprint {
        undo_failures.push("5 redos did not restore the final painted render state".to_string());
    }
    // Walk back to the base state again, so S8 starts from the document the probe
    // found (and so that S9's "restored" claim is about the *whole* probe).
    for _ in 0..5 {
        if !view.undo() {
            break;
        }
    }
    view.sync_layout();
    if view.doc().text() != base_text {
        undo_failures.push("walking back to the base state failed".to_string());
    }
    report.push_str(&format!("\n-- S7 five edits / five undos / five redos (5.3) --\n{history_rows}"));
    let name = "S7 撤销重做一致性: 5 次编辑→撤销 5 次回到初始→重做 5 次回到最终（缓冲区+块数+渲染态）";
    checks.push((
        name.to_string(),
        undo_failures.is_empty(),
        if undo_failures.is_empty() {
            format!(
                "{} bytes / {} blocks round-tripped through 5 undo and 5 redo steps, projection invariants held at every step",
                base_text.len(),
                base_blocks
            )
        } else {
            undo_failures.join("; ")
        },
    ));

    // ---------------------------------------------------------------- S8
    // The trap: undo must not record itself. Undo, redo, undo again has to walk
    // the same two states, and a new edit after an undo kills the redo branch.
    let mut edge_failures: Vec<String> = Vec::new();
    let mut edge_rows = String::new();
    view.clear_history();
    view.set_caret(target_block.min(view.doc().len_bytes()));
    view.sync_layout();
    let state0 = view.doc().text();
    view.insert_text("A");
    view.sync_layout();
    let state1 = view.doc().text();
    view.insert_text("B");
    view.sync_layout();
    let state2 = view.doc().text();
    edge_rows.push_str(&format!(
        "   three states: {} / {} / {} bytes; undo depth {} redo depth {}\n",
        state0.len(),
        state1.len(),
        state2.len(),
        view.undo_depth(),
        view.redo_depth()
    ));
    view.undo();
    view.sync_layout();
    if view.doc().text() != state1 {
        edge_failures.push("Ctrl+Z did not go back one state".to_string());
    }
    view.redo();
    view.sync_layout();
    if view.doc().text() != state2 {
        edge_failures.push("Ctrl+Y after Ctrl+Z did not return to the last state".to_string());
    }
    view.undo();
    view.sync_layout();
    if view.doc().text() != state1 {
        edge_failures.push("a second Ctrl+Z did not go back again".to_string());
    }
    if view.redo_depth() != 1 {
        edge_failures.push(format!(
            "expected exactly one redo step left, got {}",
            view.redo_depth()
        ));
    }
    // A new edit is what drops the branch.
    view.insert_text("C");
    view.sync_layout();
    edge_rows.push_str(&format!(
        "   after undo + new edit: redo depth {} (branch dropped), can_redo {}\n",
        view.redo_depth(),
        view.can_redo()
    ));
    if view.can_redo() {
        edge_failures.push("a new edit did not clear the redo branch".to_string());
    }
    if view.redo() {
        edge_failures.push("redo did something with an empty redo branch".to_string());
    }
    let mut failures = Vec::new();
    edge_rows.push_str(&crate::capture::projection_invariants(
        view,
        "after the redo-branch edge case",
        &mut failures,
    ));
    edge_failures.extend(failures);
    report.push_str(&format!("\n-- S8 撤销栈不被污染 (5.3) --\n{edge_rows}"));
    let name = "S8 撤销自身不污染撤销栈: Z→Y→Z 走同样两步，撤销后新编辑清空 redo";
    checks.push((
        name.to_string(),
        edge_failures.is_empty(),
        if edge_failures.is_empty() {
            "undo/redo move the record between the stacks; only a user edit drops the redo branch"
                .to_string()
        } else {
            edge_failures.join("; ")
        },
    ));

    // ---------------------------------------------------------------- S9
    // Leave the document as it was found, and prove it.
    let mut restore_rows = String::new();
    let mut restore_failures: Vec<String> = Vec::new();
    let mut guard = 0;
    while view.undo() {
        guard += 1;
        if guard > 256 {
            restore_failures.push("undo did not converge".to_string());
            break;
        }
    }
    view.sync_layout();
    view.clear_history();
    restore_rows.push_str(&format!(
        "   {} undo steps to get back, now {} bytes / {} blocks, undo depth {}\n",
        guard,
        view.doc().len_bytes(),
        view.doc().blocks().len(),
        view.undo_depth()
    ));
    if view.doc().text() != original_text {
        restore_failures.push(format!(
            "buffer is {} bytes, started at {}",
            view.doc().len_bytes(),
            original_text.len()
        ));
    }
    if view.doc().blocks().len() != original_blocks {
        restore_failures.push(format!(
            "blocks are {}, started at {original_blocks}",
            view.doc().blocks().len()
        ));
    }
    if render_fingerprint(view) != initial_fingerprint {
        restore_failures.push("the painted render state differs from the one we started with".to_string());
    }
    let mut failures = Vec::new();
    restore_rows.push_str(&crate::capture::projection_invariants(
        view,
        "after the probe restored the document",
        &mut failures,
    ));
    restore_failures.extend(failures);
    let layout_checks = view
        .layout()
        .map(|layout| layout.checks(view.doc()))
        .unwrap_or_default();
    for (name, ok, info) in &layout_checks {
        if !ok {
            restore_failures.push(format!("{name}: {info}"));
        }
    }
    restore_rows.push_str(&format!(
        "   {} layout checks (A1..A5) evaluated, composition left behind: {}\n",
        layout_checks.len(),
        view.marked().is_some()
    ));
    if view.marked().is_some() {
        restore_failures.push("a composition was left behind".to_string());
    }
    report.push_str(&format!("\n-- S9 the probe restores the document --\n{restore_rows}"));
    checks.push((
        "S9 the probe restores the document and leaves no composition behind".to_string(),
        restore_failures.is_empty(),
        if restore_failures.is_empty() {
            format!(
                "{} bytes / {} blocks / same render state, {} layout checks green",
                original_text.len(),
                original_blocks,
                layout_checks.len()
            )
        } else {
            restore_failures.join("; ")
        },
    ));

    // A datum for group 7's viewport work: highlight geometry costs two
    // `Metrics::x_for_index` calls per covered fragment, and
    // `WindowMetrics::x_for_index` shape-forms the fragment on every call. This is
    // what a Ctrl+A frame would pay today.
    view.select_all();
    view.sync_layout();
    let started = std::time::Instant::now();
    let all = view.selection_rects();
    let select_all_ms = started.elapsed().as_secs_f64() * 1000.;
    report.push_str(&format!(
        "\n-- select-all highlight cost (datum for task 7.1) --\n   {} rectangles over {} blocks in {:.3} ms, shaping not cached\n",
        all.len(),
        view.doc().blocks().len(),
        select_all_ms
    ));

    // The highlight is painted by `prepaint`, which ran before this probe. Leave a
    // live selection and ask for one more frame: if painting a selection panicked,
    // the process would die here instead of exiting 0, so a passing run is also
    // evidence that the highlight paint path is not a landmine.
    if let Some(sel) = cases.three_fragments.or(cases.whole_paragraph) {
        view.select(sel.anchor, sel.head);
        view.sync_layout();
        report.push_str(&format!(
            "\n-- one more frame with a live selection --\n   selection {:?}, {} highlight quads, repaint requested\n",
            sel.src_range(),
            view.selection_rects().len()
        ));
        cx.notify();
    }

    ProbeCapture { report, checks }
}

// ---------------------------------------------------------------- unit tests

#[cfg(test)]
mod tests {
    // No glob import of gpui here: it exports a `test` module, which would shadow
    // the `#[test]` attribute (the expansion recurses into that module).
    use std::sync::Arc;

    use gpui_kit::gpui::{point, px};

    use super::{
        History, HistoryAction, Selection, display_ranges, highlight_rects, inverse, seam_gaps,
        three_fragment_selection, undo_range,
    };
    use typors_core::Document;
    use typors_render::{ApproxMetrics, DocLayout, EditorTheme, LayoutMode};
    use crate::view::EditRecord;

    fn layout(text: &str, mode: LayoutMode) -> (Document, DocLayout) {
        let doc = Document::parse(text);
        let layout = DocLayout::layout_mode(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            mode,
        );
        (doc, layout)
    }

    fn record(src: std::ops::Range<usize>, removed: &str, inserted: &str) -> EditRecord {
        EditRecord {
            src,
            removed: removed.to_string(),
            inserted: inserted.to_string(),
            caret_before: Some(0),
            caret_after: Some(0),
            revision: 1,
        }
    }

    #[test]
    fn selection_text_is_the_contiguous_source_slice_with_markers() {
        let (doc, _) = layout(super::SAMPLE, LayoutMode::RENDERED);
        let sel = three_fragment_selection(&doc).expect("the sample has 粗体 and 代码");
        let copied = sel.text(&doc).expect("non-empty selection");
        // Continuous source bytes: the emphasis and code markers are part of it,
        // because the selection is a byte range of the buffer and nothing else.
        assert!(copied.contains("**"), "copied {copied:?} lost the ** markers");
        assert!(copied.contains('`'), "copied {copied:?} lost the backtick");
        assert_eq!(copied, doc.text()[sel.src_range()].to_string());
        assert!(!sel.is_reversed());
    }

    #[test]
    fn highlight_rects_tile_each_visual_line_and_cover_every_fragment() {
        let (doc, layout) = layout(super::SAMPLE, LayoutMode::RENDERED);
        let sel = three_fragment_selection(&doc).expect("selection");
        let rects = highlight_rects(&doc, &layout, sel.src_range());
        // bold run, the plain text between, and the code run: at least three
        // separate fragments on one visual line.
        assert!(rects.len() >= 3, "only {} rectangles", rects.len());
        assert_eq!(seam_gaps(&rects), Vec::<f32>::new(), "{rects:?}");
        // The union starts and ends where the selection does, measured by the
        // independent `caret_x` path.
        let (block_idx, display) = display_ranges(&doc, &layout, sel.src_range())[0].clone();
        let start_x = layout.caret_x(block_idx, display.start).expect("start x").0;
        let end_x = layout.caret_x(block_idx, display.end).expect("end x").0;
        assert!((rects[0].left - start_x.as_f32()).abs() < 0.01);
        assert!((rects[rects.len() - 1].right - end_x.as_f32()).abs() < 0.01);
        for rect in &rects {
            let bounds = layout.block_bounds(rect.block_idx).expect("block bounds");
            assert!(rect.line_top >= bounds.origin.y.as_f32() - 0.5);
            assert!(rect.line_top + rect.line_height <= bounds.bottom().as_f32() + 0.5);
        }
    }

    #[test]
    fn a_marker_only_selection_has_no_rect_but_keeps_its_bytes() {
        let (doc, layout) = layout(super::SAMPLE, LayoutMode::RENDERED);
        // `# ` is a zero-width marker: nothing is painted there, so nothing can be
        // highlighted -- but the bytes are still copyable.
        let marker = Selection::new(0, 2);
        assert!(highlight_rects(&doc, &layout, marker.src_range()).is_empty());
        assert_eq!(marker.text(&doc).as_deref(), Some("# "));
    }

    #[test]
    fn undo_record_inverts_a_splice_both_ways() {
        let mut doc = Document::parse("abcdef");
        doc.apply_edit(2..4, "XY");
        assert_eq!(doc.text(), "abXYef");
        let edit = record(2..4, "cd", "XY");
        assert_eq!(undo_range(&edit), 2..4);
        // The undo is `apply_edit(undo_range, removed)` -- exactly the recipe the
        // view runs.
        doc.apply_edit(undo_range(&edit), &edit.removed);
        assert_eq!(doc.text(), "abcdef");
        // And the redo is the record itself, with the two carets swapped by
        // `inverse`.
        let back = inverse(&edit);
        assert_eq!(back.src, 2..4);
        assert_eq!(back.removed, "XY");
        doc.apply_edit(back.src.clone(), &back.removed);
        assert_eq!(doc.text(), "abXYef");
    }

    #[test]
    fn history_drops_the_redo_branch_only_for_user_edits() {
        let mut history = History::new();
        assert!(!history.can_undo() && !history.can_redo());

        history.record(record(0..0, "", "a"));
        history.record(record(1..1, "", "b"));
        assert_eq!(history.undo_depth(), 2);
        assert_eq!(history.redo_depth(), 0);

        let undone = history.pop_undo().expect("one undo step");
        history.push_redo(undone);
        assert_eq!((history.undo_depth(), history.redo_depth()), (1, 1));

        // A replay must not touch either stack: that is the anti-pollution rule.
        // Only the caller moves records, and the redo branch survives a redo.
        let redone = history.pop_redo().expect("one redo step");
        history.push_undo_again(redone);
        assert_eq!((history.undo_depth(), history.redo_depth()), (2, 0));

        let undone = history.pop_undo().expect("undo again");
        history.push_redo(undone);
        assert_eq!((history.undo_depth(), history.redo_depth()), (1, 1));
        // A new user edit is what kills the redo branch.
        history.record(record(0..1, "a", "c"));
        assert_eq!((history.undo_depth(), history.redo_depth()), (2, 0));

        history.clear();
        assert_eq!((history.undo_depth(), history.redo_depth()), (0, 0));
        // `HistoryAction` exists so the call site has to say which one it means.
        assert_ne!(HistoryAction::Record, HistoryAction::Replay);
    }
}
