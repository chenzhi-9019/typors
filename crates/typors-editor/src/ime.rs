//! Group 6: the Windows IME channel (H2), and the composition buffer behind it.
//!
//! # The model (design.md D4)
//!
//! The compilation string never enters the rope. [`MarkedText`] holds it plus the
//! source offset it would be inserted at; [`typors_render::DocLayout`] splices it
//! into the caret block's source projection so the composition has a geometry to
//! be drawn at, and `bounds_for_range` can answer the platform in the same
//! coordinate system the string is painted in.
//!
//! Two facts from the source-level probe drive every decision here
//! (`docs/spike/gpui-api-probe.md` §3.4):
//!
//! * `WM_IME_ENDCOMPOSITION` is **not** handled by `gpui-pre-windows`, and
//!   `unmark_text` / `marked_text_range` are never called on Windows. Nothing
//!   tells us a composition ended, so every path that ends one (`replace_text_in_range`
//!   commit or cancel, `unmark_text` on other platforms) clears `marked` itself.
//! * The platform always passes `range = None` to
//!   `replace_and_mark_text_in_range` / `replace_text_in_range`
//!   (`gpui-pre-windows/src/events.rs:720-754`), so the anchor has to come from
//!   our own caret, not from the incoming range.
//!
//! # Coordinates
//!
//! Three coordinate systems meet here, and getting them confused is the whole
//! risk of 6.2:
//!
//! | system | who speaks it | unit |
//! | --- | --- | --- |
//! | buffer | `ropey`, AST, `apply_edit` | UTF-8 bytes |
//! | composite | `EntityInputHandler` while a composition is live | UTF-8 bytes, composition spliced in |
//! | display | `DrawSpan::render`, `DocLayout::caret_bounds` | UTF-8 bytes inside one block's painted text |
//!
//! [`MarkedText::to_source`] / [`MarkedText::to_composite`] convert buffer <->
//! composite; `DocLayout::display_byte_for_composite` finishes the way to display.

use std::ops::Range;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::gpui::*;

use typors_core::doc::{Document, Style};
use crate::capture::ProbeCapture;
use typors_render::{BlockMode, DocLayout};
use crate::view::{byte_to_utf16, utf16_to_byte, MarkdownView};

pub use typors_core::MarkedText;

// ---------------------------------------------------------------- logging

fn stamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("t=+{:.3}s wall={}ms", start.elapsed().as_secs_f64(), wall)
}

/// One stderr line per IME callback, for the manual pass on a real IME.
///
/// A reader of the log has to answer three questions, which are exactly the three
/// breakpoints of `findings.md` H2:
///
/// * **do composition events arrive at all** — look for `mark` / `start` lines
///   while typing pinyin. No `mark` line while the candidate window is up means
///   the composition never reached the app;
/// * **is the candidate window on the caret** — compare the `bounds` line's
///   `x,y` against where the候选窗 actually appeared;
/// * **did the commit lose characters** — compare the `commit` line's `result`
///   with `rope` (`revision` and byte count) and the next `bounds` line.
///
/// `TYPORS_IME_LOG=off` silences it.
pub fn log_ime(event: &str, detail: std::fmt::Arguments<'_>) {
    if matches!(std::env::var("TYPORS_IME_LOG").as_deref(), Ok("off") | Ok("0")) {
        return;
    }
    eprintln!("ime {} {event:<14} {detail}", stamp());
}

/// `marked` / rope / caret summary appended to every log line, so one line says
/// both what came in and what state it left behind.
pub fn state(view: &MarkdownView) -> String {
    let marked = match view.marked() {
        Some(m) => match &m.selection {
            Some(sel) => format!("{:?}@{} sel={sel:?}", m.text, m.anchor),
            None => format!("{:?}@{}", m.text, m.anchor),
        },
        None => "none".to_string(),
    };
    format!(
        "marked={marked} rope#{}/{}B caret={}",
        view.doc().revision(),
        view.doc().len_bytes(),
        view.caret()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string()),
    )
}

// ---------------------------------------------------------------- probe

/// What the *composite* layout says about one byte offset.
///
/// Deliberately reports the painted fragment's own origin instead of only the
/// caret rectangle: an assertion that compares `bounds_for_range` against
/// `caret_bounds` would be the layout agreeing with itself, while the fragment
/// origin comes from the pen advance in `Flow::layout_lines` — an independent
/// code path.
struct CompositePos {
    block_idx: usize,
    display_byte: usize,
    mode: BlockMode,
    painted: String,
    /// Origin of the fragment that owns `display_byte`.
    span_origin: Option<Point<Pixels>>,
    /// Where the fragment *ending* at `display_byte` stops (its right edge, on
    /// its own line). At a soft wrap the caret is legitimately drawn either at
    /// the end of the first line or at the start of the next, so the probe
    /// accepts both edges instead of encoding one convention.
    edge_before: Option<Point<Pixels>>,
    span_underline: bool,
    span_text: String,
    span_font_size: Pixels,
    span_style: Style,
    /// Line box height of the block that owns it (a `Heading { level: 2 }` line is
    /// 34.3 px while a body line is 26.4 px).
    line_height: Pixels,
    /// True when a fragment starts exactly at `display_byte`.
    starts_fragment: bool,
}

fn composite_pos(layout: &DocLayout, doc: &Document, composite: usize) -> Option<CompositePos> {
    let (block_idx, display_byte) = layout.display_byte_for_composite(doc, composite)?;
    let block = layout.blocks.get(block_idx)?;
    let painted = block.painted_text();
    // A fragment that *starts* here wins over one that merely ends here: at a
    // fragment boundary both match, and the composition is the later one.
    let exact = block
        .spans
        .iter()
        .find(|s| s.render.start == display_byte);
    let span = exact.or_else(|| {
        block
            .spans
            .iter()
            .find(|s| s.render.start <= display_byte && display_byte <= s.render.end)
    });
    let edge_before = block
        .spans
        .iter()
        .find(|s| s.render.end == display_byte)
        .map(|s| point(px(s.origin.x.as_f32() + s.width.as_f32()), s.origin.y));
    Some(CompositePos {
        block_idx,
        display_byte,
        mode: block.mode,
        painted,
        span_origin: span.map(|s| s.origin),
        edge_before,
        span_underline: span.map(|s| s.underline).unwrap_or(false),
        span_text: span.map(|s| s.text.clone()).unwrap_or_default(),
        span_font_size: span.map(|s| s.font_size).unwrap_or(px(17.)),
        span_style: span.map(|s| s.style).unwrap_or(Style::PLAIN),
        line_height: block.line_height,
        starts_fragment: exact.is_some(),
    })
}

fn source_blocks(layout: &DocLayout) -> Vec<usize> {
    layout
        .blocks
        .iter()
        .filter(|b| b.mode == BlockMode::Source)
        .map(|b| b.block_idx)
        .collect()
}

/// `text` with `range` replaced — computed without the editor, so "the buffer is
/// exactly what the IME should have produced" is an independent claim.
fn spliced(text: &str, range: Range<usize>, with: &str) -> String {
    let start = range.start.min(text.len());
    let end = range.end.clamp(start, text.len());
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..start]);
    out.push_str(with);
    out.push_str(&text[end..]);
    out
}

fn close(a: Pixels, b: Pixels, tolerance: f32) -> bool {
    (a.as_f32() - b.as_f32()).abs() <= tolerance
}

fn fmt_bounds(rect: Option<Bounds<Pixels>>) -> String {
    match rect {
        Some(rect) => format!(
            "({:.1},{:.1} {:.1}x{:.1})",
            rect.origin.x.as_f32(),
            rect.origin.y.as_f32(),
            rect.size.width.as_f32(),
            rect.size.height.as_f32()
        ),
        None => "(none)".to_string(),
    }
}

fn step_line(view: &MarkdownView, name: &str, extra: &str) -> String {
    format!("   {name:<22} {extra:<30} {}\n", state(view))
}

/// A check whose precondition this document does not meet; not a pass claim.
fn not_exercised(name: &str, why: &str) -> (String, bool, String) {
    (name.to_string(), true, format!("not exercised: {why}"))
}

/// `--dump-ime`: replay the exact callback sequence the Windows IME path produces
/// (`gpui-pre-windows/src/events.rs:720-754`) against the frame that was just
/// painted, and assert 6.1-6.4 on the result.
///
/// This cannot replace a human typing pinyin on a real IME — nothing here proves
/// `WM_IME_COMPOSITION` ever arrives. What it does prove is that *if* the platform
/// delivers the callbacks, the composition model answers correctly: the rope does
/// not move until commit, the composition is painted at the caret, the candidate
/// anchor rectangle is the caret's, and commit / cancel / ASCII-after-commit land
/// byte for byte. `findings.md` H2 keeps the verdict at「待人工确认」 for exactly
/// that reason.
pub fn ime_probe(
    view: &mut MarkdownView,
    window: &mut Window,
    cx: &mut Context<MarkdownView>,
) -> ProbeCapture {
    view.ensure_layout();
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut report = String::new();
    let original = view.doc().text();
    let original_rev = view.doc().revision();
    report.push_str(&format!(
        "# ime probe: {} bytes, {} blocks, measured by {}\n\
         # replaying the Windows callback order: replace_and_mark_text_in_range(compstr)\n\
         #   -> replace_text_in_range(result) / replace_text_in_range(\"\") for cancel\n",
        original.len(),
        view.doc().blocks().len(),
        view.layout()
            .map(|l| l.metrics_label)
            .unwrap_or("<no layout>")
    ));

    // The document the window is painting. `element_bounds` the platform passes
    // in is a rectangle it computed itself; 6.2 forbids using it for the anchor,
    // so the probe hands in two very different ones and expects one answer.
    let element_bounds = Bounds::new(
        point(px(0.), px(0.)),
        size(px(1472.), px(2400.)),
    );
    let bogus_element_bounds = Bounds::new(
        point(px(411.), px(733.)),
        size(px(90.), px(4000.)),
    );

    let paragraph = view.layout().and_then(|layout| {
        layout
            .blocks
            .iter()
            .find(|b| b.kind == typors_core::doc::BlockKind::Paragraph && !b.spans.is_empty())
            .map(|b| (b.block_idx, b.src.clone()))
    });

    // ---------------------------------------------------------------- I1 / I2
    let mut failures: Vec<String> = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    // Carried out of the block for I3..I6.
    let mut anchor = 0usize;

    match &paragraph {
        Some((p_idx, p_src)) => {
            let start = view.doc().snap_floor(p_src.start + 3).max(p_src.start);
            view.set_caret(start);
            view.sync_layout();
            let start = view.caret().unwrap_or(start);
            anchor = start;
            let rope_before = view.doc().text();
            let rev_before = view.doc().revision();

            // 6.1: the composition string arrives, the rope does not move.
            view.replace_and_mark_text_in_range(None, "nihao", Some(0..5), window, cx);
            let relayout = std::time::Instant::now();
            view.sync_layout();
            let relayout_ms = relayout.elapsed().as_secs_f64() * 1000.;
            detail.push_str(&step_line(view, "1 start nihao", "compstr=nihao sel=0..5"));
            // Data for group 7 / H1: an IME update repaints through a *full*
            // relayout, exactly like a keystroke, so 10 万字符文档 pays the same
            // 10-11 ms per pinyin keystroke.
            detail.push_str(&format!(
                "   relayout per update      {relayout_ms:.3} ms (full document)\n"
            ));
            match view.marked() {
                Some(m) if m.text == "nihao" => detail.push_str(&format!(
                    "   composition buffer        {:?} at src {} ({} bytes)\n",
                    m.text, m.anchor, m.len()
                )),
                other => failures.push(format!("composition buffer after start is {other:?}")),
            }
            if view.doc().text() != rope_before {
                failures.push("composition wrote to the rope (6.1 violated)".to_string());
            }
            if view.doc().revision() != rev_before {
                failures.push(format!(
                    "composition bumped the rope revision {} -> {}",
                    rev_before,
                    view.doc().revision()
                ));
            }
            let start_pos = view
                .layout()
                .and_then(|layout| composite_pos(layout, view.doc(), start));
            match &start_pos {
                Some(pos) => {
                    detail.push_str(&format!(
                        "   composite layout         block {} {:?} display_byte={} painted={:?}\n",
                        pos.block_idx,
                        pos.mode,
                        pos.display_byte,
                        truncate(&pos.painted, 60)
                    ));
                    if pos.block_idx != *p_idx {
                        failures.push(format!(
                            "composition landed in block {} instead of paragraph {}",
                            pos.block_idx, p_idx
                        ));
                    }
                    // The composition is painted, at the caret, underlined.
                    if pos.painted.get(pos.display_byte..pos.display_byte + 5) != Some("nihao") {
                        failures.push(format!(
                            "composition is not painted at display byte {}: {:?}",
                            pos.display_byte, pos.painted
                        ));
                    }
                    if !pos.starts_fragment || !pos.span_underline {
                        failures.push(format!(
                            "the composition fragment is not its own underlined run (starts={} underline={})",
                            pos.starts_fragment, pos.span_underline
                        ));
                    }
                    if !pos.span_text.starts_with("nihao") {
                        failures.push(format!(
                            "the fragment at the composition start is {:?}",
                            pos.span_text
                        ));
                    }
                }
                None => failures.push("no composite geometry for the composition start".to_string()),
            }

            // 6.1 / candidate anchor: the rectangle the platform would hand to
            // `ImmSetCandidateWindow` must be the composition's own fragment.
            if let Some(sel) = view.selected_text_range(false, window, cx) {
                let bounds = view.bounds_for_range(sel.range.clone(), element_bounds, window, cx);
                // The platform asks about the caret, and while composing that is
                // the IME's own cursor inside the composition string, so resolve
                // the queried range in composite coordinates.
                let composite = view.marked().map(|m| m.splice(&view.doc().text(), 0).0);
                let queried = composite.map(|text| {
                    utf16_to_byte(&text, sel.range.start)..utf16_to_byte(&text, sel.range.end)
                });
                let pos = queried.clone().and_then(|range| {
                    view.layout()
                        .and_then(|layout| composite_pos(layout, view.doc(), range.start))
                });
                detail.push_str(&format!(
                    "   selected_text_range       utf16={:?} composite={queried:?} -> bounds_for_range {}\n",
                    sel.range,
                    fmt_bounds(bounds)
                ));
                match (bounds, pos) {
                    (Some(bounds), Some(pos)) => {
                        // The anchor must sit on a fragment edge next to the
                        // queried offset: the start of the following fragment, or
                        // the end of the preceding one (a soft wrap puts the
                        // caret legitimately on either).
                        let on_edge = [pos.span_origin, pos.edge_before]
                            .into_iter()
                            .flatten()
                            .any(|edge| {
                                close(bounds.origin.x, edge.x, 0.5)
                                    && close(bounds.origin.y, edge.y, 0.5)
                            });
                        if !on_edge {
                            failures.push(format!(
                                "bounds_for_range {} is on no fragment edge of block {} at display byte {} (following {:?}, preceding {:?})",
                                fmt_bounds(Some(bounds)),
                                pos.block_idx,
                                pos.display_byte,
                                pos.span_origin.map(|p| (p.x.as_f32(), p.y.as_f32())),
                                pos.edge_before.map(|p| (p.x.as_f32(), p.y.as_f32())),
                            ));
                        }
                        if pos.block_idx != *p_idx {
                            failures.push(format!(
                                "bounds_for_range resolved to block {}, expected paragraph {}",
                                pos.block_idx, p_idx
                            ));
                        }
                        // The candidate window anchors on the middle of this
                        // rectangle (`events.rs:641-642`), so the line box height
                        // is part of the contract, not decoration.
                        if !close(bounds.size.height, pos.line_height, 0.5) {
                            failures.push(format!(
                                "bounds_for_range height {:.1} != line height {:.1}",
                                bounds.size.height.as_f32(),
                                pos.line_height.as_f32()
                            ));
                        }
                    }
                    (None, _) => {
                        failures.push("bounds_for_range returned nothing while composing".to_string())
                    }
                    (_, None) => failures.push("no fragment at the queried caret".to_string()),
                }
            } else {
                failures.push("selected_text_range returned None while composing".to_string());
            }

            // 6.1: an update replaces the string, it does not append to it.
            view.replace_and_mark_text_in_range(None, "nih", None, window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "2 update nih", "compstr=nih sel=none"));
            if view.doc().text() != rope_before || view.doc().revision() != rev_before {
                failures.push("composition update wrote to the rope".to_string());
            }
            match view.marked().map(|m| m.text.as_str()) {
                Some("nih") => {}
                other => failures.push(format!("composition buffer after update is {other:?}")),
            }
            if let Some(pos) = view
                .layout()
                .and_then(|layout| composite_pos(layout, view.doc(), start))
            {
                if pos.painted.get(pos.display_byte..pos.display_byte + 3) != Some("nih") {
                    failures.push(format!(
                        "updated composition is not painted at display byte {}: {:?}",
                        pos.display_byte, pos.painted
                    ));
                }
                if pos.painted.contains("nihao") || pos.painted.contains("nihan") {
                    failures.push(format!(
                        "composition update appended instead of replacing: {:?}",
                        pos.painted
                    ));
                }
            }
        }
        None => skipped = Some("no paragraph block with painted fragments".to_string()),
    }
    report.push_str(&format!("\n-- I1/I2 composition start and update.\n{detail}"));
    let name = "I1/I2 composition is displayed without touching the buffer, update replaces it";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            format!(
                "rope unchanged at revision {}, composition painted at the caret and underlined",
                original_rev
            ),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- I3 / I6
    let mut failures: Vec<String> = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    let mut after_commit = String::new();
    let mut caret_after_commit = 0usize;
    let mut revision_after_commit = 0u64;
    match &paragraph {
        Some((p_idx, _)) => {
            let before = view.doc().text();
            let rev_before = view.doc().revision();
            // 6.3: GCS_RESULTSTR -> replace_text_in_range(None, result). One call,
            // one edit, four characters, caret right behind them.
            view.replace_text_in_range(None, "你好世界", window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "3 commit 你好世界", "result=你好世界 range=none"));
            let expected = spliced(&before, anchor..anchor, "你好世界");
            if view.doc().text() != expected {
                failures.push("commit did not insert the result at the composition anchor".to_string());
            }
            if view.doc().revision() != rev_before + 1 {
                failures.push(format!(
                    "commit took {} revisions, expected 1",
                    view.doc().revision() - rev_before
                ));
            }
            if view.marked().is_some() {
                failures.push("the composition survived the commit (no unmark_text on Windows)".to_string());
            }
            let caret_expected = anchor + "你好世界".len();
            if view.caret() != Some(caret_expected) {
                failures.push(format!(
                    "caret after commit is {:?}, expected {}",
                    view.caret(),
                    caret_expected
                ));
            }
            after_commit = view.doc().text();
            caret_after_commit = view.caret().unwrap_or(caret_expected);
            revision_after_commit = view.doc().revision();

            // 6.3 / spec: the paragraph is a rendered block again once the
            // composition is gone -- the caret block is the only source block.
            let post = view
                .layout()
                .and_then(|layout| composite_pos(layout, view.doc(), anchor));
            match &post {
                Some(pos) => {
                    detail.push_str(&format!(
                        "   after commit             block {} {:?} painted={:?}\n",
                        pos.block_idx,
                        pos.mode,
                        truncate(&pos.painted, 60)
                    ));
                    if !pos.painted.contains("你好世界") {
                        failures.push(format!(
                            "committed text is not painted in block {}: {:?}",
                            pos.block_idx, pos.painted
                        ));
                    }
                    if pos.painted.matches("你好世界").count() != 1 {
                        failures.push(format!(
                            "committed text appears {} times in the block",
                            pos.painted.matches("你好世界").count()
                        ));
                    }
                    if pos.painted.contains("nih") {
                        failures.push(format!("composition residue after commit: {:?}", pos.painted));
                    }
                    if pos.block_idx != *p_idx {
                        failures.push(format!(
                            "after commit the caret block is {} not paragraph {}",
                            pos.block_idx, p_idx
                        ));
                    }
                }
                None => failures.push("no geometry after commit".to_string()),
            }
            let sources = view.layout().map(source_blocks).unwrap_or_default();
            if sources != vec![*p_idx] {
                failures.push(format!(
                    "source projection after commit is {sources:?}, expected [{p_idx}]"
                ));
            }
            let rendered_siblings = view
                .layout()
                .map(|layout| {
                    layout
                        .blocks
                        .iter()
                        .filter(|b| b.block_idx != *p_idx && b.mode == BlockMode::Rendered)
                        .count()
                })
                .unwrap_or(0);
            detail.push_str(&format!(
                "   projection               {rendered_siblings} sibling blocks still rendered\n"
            ));

            // 6.2 with CJK: the rectangle of the four committed characters has to
            // carry their real advance, which is roughly twice the ASCII advance
            // of the same character count.
            let cjk_inputs = view.layout().map(|layout| {
                let font_size = post
                    .as_ref()
                    .map(|p| p.span_font_size)
                    .unwrap_or(px(17.));
                let style = post.as_ref().map(|p| p.span_style).unwrap_or(Style::PLAIN);
                (layout.metrics.clone(), font_size, style)
            });
            if let Some((metrics, font_size, style)) = cjk_inputs {
                let utf16 = byte_to_utf16(&view.doc().text(), anchor)
                    ..byte_to_utf16(&view.doc().text(), caret_expected);
                let bounds = view.bounds_for_range(utf16, element_bounds, window, cx);
                let cjk = metrics.advance("你好世界", font_size, style);
                let ascii = metrics.advance("abcd", font_size, style);
                detail.push_str(&format!(
                    "   CJK range bounds         {} measured cjk={:.1} ascii4={:.1}\n",
                    fmt_bounds(bounds),
                    cjk.as_f32(),
                    ascii.as_f32()
                ));
                match bounds {
                    Some(bounds) => {
                        if !(bounds.size.width.as_f32() > 1.5 * ascii.as_f32()
                            && bounds.size.width.as_f32() < 2.5 * ascii.as_f32())
                        {
                            failures.push(format!(
                                "CJK range width {:.1} is not ~2x the ASCII width {:.1}",
                                bounds.size.width.as_f32(),
                                ascii.as_f32()
                            ));
                        }
                        if (bounds.size.width.as_f32() - cjk.as_f32()).abs() > 3.0 {
                            failures.push(format!(
                                "CJK range width {:.1} does not match the measured advance {:.1}",
                                bounds.size.width.as_f32(),
                                cjk.as_f32()
                            ));
                        }
                    }
                    None => failures.push("bounds_for_range returned nothing for the CJK range".to_string()),
                }
            }

            // 6.2: whatever `element_bounds` the platform passes must not move the
            // answer. This is the double-offset bug the layout already avoids.
            let real = view.bounds_for_range(
                byte_to_utf16(&view.doc().text(), anchor)
                    ..byte_to_utf16(&view.doc().text(), caret_expected),
                element_bounds,
                window,
                cx,
            );
            let bogus = view.bounds_for_range(
                byte_to_utf16(&view.doc().text(), anchor)
                    ..byte_to_utf16(&view.doc().text(), caret_expected),
                bogus_element_bounds,
                window,
                cx,
            );
            detail.push_str(&format!(
                "   element_bounds ignored   {} vs {} (element bounds provided)\n",
                fmt_bounds(real),
                fmt_bounds(bogus)
            ));
            match (real, bogus) {
                (Some(a), Some(b)) => {
                    if !close(a.origin.x, b.origin.x, 0.01) || !close(a.origin.y, b.origin.y, 0.01) {
                        failures.push(format!(
                            "element_bounds moved the candidate anchor: {} != {}",
                            fmt_bounds(Some(a)),
                            fmt_bounds(Some(b))
                        ));
                    }
                }
                (None, _) | (_, None) => {
                    failures.push("bounds_for_range returned nothing for the committed range".to_string())
                }
            }
        }
        None => skipped = Some("no paragraph block to commit into".to_string()),
    }
    report.push_str(&format!("\n-- I3/I6 commit, CJK geometry and element_bounds.\n{detail}"));
    let name = "I3/I6 commit inserts once at the anchor, CJK width is measured, element_bounds ignored";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "one edit, caret behind the four characters, block still the only source block"
                .to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- I4 / I5
    let mut failures: Vec<String> = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    match &paragraph {
        Some(_) if !after_commit.is_empty() => {
            let rev_before = view.doc().revision();

            // Esc: the platform reports an empty composition, then (with
            // `lparam == 0`) `replace_text_in_range(None, "")`.
            view.replace_and_mark_text_in_range(None, "nihao", Some(0..5), window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "4 start again nihao", "compstr=nihao"));
            if view.doc().text() != after_commit || view.doc().revision() != rev_before {
                failures.push("composition before the cancel already changed the buffer".to_string());
            }
            view.replace_text_in_range(None, "", window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "4 cancel Esc", "result=\"\" range=none"));
            if view.doc().text() != after_commit {
                failures.push(format!(
                    "cancel changed the buffer: {} bytes -> {} bytes",
                    after_commit.len(),
                    view.doc().text().len()
                ));
            }
            if view.doc().revision() != rev_before {
                failures.push(format!(
                    "cancel recorded an edit: revision {} -> {}",
                    rev_before,
                    view.doc().revision()
                ));
            }
            if view.marked().is_some() {
                failures.push("the composition survived the cancel".to_string());
            }
            if view.caret() != Some(caret_after_commit) {
                failures.push(format!(
                    "caret after cancel is {:?}, expected {}",
                    view.caret(),
                    caret_after_commit
                ));
            }

            // 6.3, second scenario: ASCII right after the commit, through WM_CHAR.
            view.replace_text_in_range(None, "abc ", window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "5 ascii 'abc '", "chars=abc+space"));
            let expected = spliced(&after_commit, caret_after_commit..caret_after_commit, "abc ");
            if view.doc().text() != expected {
                failures.push("'abc ' after the commit did not append cleanly".to_string());
            }
            if view.caret() != Some(caret_after_commit + 4) {
                failures.push(format!(
                    "caret after 'abc ' is {:?}, expected {}",
                    view.caret(),
                    caret_after_commit + 4
                ));
            }
            if view.doc().text().matches("你好世界").count() != 1 {
                failures.push("the committed word was duplicated".to_string());
            }
            if view.doc().text().contains("你好世界nihao") || view.doc().text().contains("niha") {
                failures.push("composition residue in the buffer".to_string());
            }
            detail.push_str(&format!(
                "   buffer prefix            {:?}\n",
                truncate(&view.doc().text(), 80)
            ));
            if view.doc().revision() <= revision_after_commit {
                failures.push("'abc ' did not go through a single edit".to_string());
            }
        }
        _ => skipped = Some("no committed text to cancel or append after".to_string()),
    }
    report.push_str(&format!("\n-- I4/I5 cancel and ASCII right after the commit.\n{detail}"));
    let name = "I4/I5 cancel leaves the buffer byte-identical, ASCII after commit appends cleanly";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "cancel: same bytes, same revision, caret restored; 'abc ' appended once".to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- I7
    let mut failures: Vec<String> = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    let heading = view.layout().and_then(|layout| {
        layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, typors_core::doc::BlockKind::Heading { level } if level == 2))
            .map(|b| (b.block_idx, b.src.clone(), b.line_height))
    });
    match (&paragraph, heading) {
        (Some((p_idx, _)), Some((h_idx, h_src, h_line_height))) => {
            // Before the caret arrives the heading is a rendered block; the
            // composition must only ever see it as source.
            view.set_caret(view.doc().snap_floor(h_src.start + 3));
            view.sync_layout();
            let was_rendered = view
                .layout()
                .and_then(|layout| layout.blocks.iter().find(|b| b.block_idx == *p_idx))
                .map(|b| b.mode);
            detail.push_str(&format!(
                "   caret into the heading   paragraph {p_idx} is now {was_rendered:?}, heading {h_idx} is {:?}\n",
                view.layout().and_then(|l| l.block_mode(h_idx))
            ));
            if was_rendered != Some(BlockMode::Rendered) {
                failures.push("the paragraph did not go back to the render projection".to_string());
            }

            let expect_source = |step: &str,
                                     failures: &mut Vec<String>,
                                     view: &MarkdownView,
                                     detail: &mut String| {
                let sources = view.layout().map(source_blocks).unwrap_or_default();
                let mode = view.layout().and_then(|l| l.block_mode(h_idx));
                let painted = view
                    .layout()
                    .and_then(|l| l.blocks.iter().find(|b| b.block_idx == h_idx))
                    .map(|b| b.painted_text())
                    .unwrap_or_default();
                detail.push_str(&format!(
                    "   projection @ {step:<12} heading {:?} source blocks {sources:?} painted {:?}\n",
                    mode,
                    truncate(&painted, 46)
                ));
                if mode != Some(BlockMode::Source) {
                    failures.push(format!("the heading left the source projection at {step}"));
                }
                if sources != vec![h_idx] {
                    failures.push(format!("source blocks at {step} are {sources:?}, expected [{h_idx}]"));
                }
                if !painted.starts_with("## ") {
                    failures.push(format!("markers are not painted during composition at {step}"));
                }
            };
            expect_source("composition start", &mut failures, view, &mut detail);

            view.replace_and_mark_text_in_range(None, "biaoti", Some(0..6), window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "7 heading compstr", "compstr=biaoti sel=0..6"));
            expect_source("after start", &mut failures, view, &mut detail);

            // The 34.3 px heading line: the candidate anchor must sit on it, not
            // on a body line height.
            let sel = view.selected_text_range(false, window, cx);
            if let Some(sel) = sel
                && let Some(bounds) = view.bounds_for_range(sel.range.clone(), element_bounds, window, cx)
            {
                detail.push_str(&format!(
                    "   heading anchor           bounds {} line height {:.1}\n",
                    fmt_bounds(Some(bounds)),
                    h_line_height.as_f32()
                ));
                if !close(bounds.size.height, h_line_height, 0.5) {
                    failures.push(format!(
                        "heading candidate anchor height {:.1} != heading line height {:.1}",
                        bounds.size.height.as_f32(),
                        h_line_height.as_f32()
                    ));
                }
                if bounds.size.height.as_f32() <= 26.4 {
                    failures.push(format!(
                        "heading candidate anchor uses the body line height {:.1}",
                        bounds.size.height.as_f32()
                    ));
                }
            } else {
                failures.push("no candidate anchor for the heading composition".to_string());
            }

            view.replace_and_mark_text_in_range(None, "biaotilan", None, window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "7 heading update", "compstr=biaotilan sel=none"));
            expect_source("after update", &mut failures, view, &mut detail);

            view.replace_text_in_range(None, "标题蓝", window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "7 heading commit", "result=标题蓝"));
            expect_source("after commit", &mut failures, view, &mut detail);
            if view.caret_block() != Some(h_idx) {
                failures.push(format!(
                    "caret left the heading after commit: {:?}",
                    view.caret_block()
                ));
            }
            // And out again: moving the caret away restores the render projection.
            view.set_caret(view.doc().snap_floor(h_src.start + 9).max(h_src.start));
            view.sync_layout();
            view.set_caret(0);
            view.sync_layout();
            let after = view.layout().and_then(|l| l.block_mode(h_idx));
            detail.push_str(&format!(
                "   caret moved out          heading is now {:?}\n",
                after
            ));
            if after != Some(BlockMode::Rendered) {
                failures.push(format!("the heading stayed in the source projection: {after:?}"));
            }
        }
        (None, _) => skipped = Some("no paragraph block".to_string()),
        (_, None) => skipped = Some("no level-2 heading block".to_string()),
    }
    report.push_str(&format!("\n-- I7 the caret block stays in the source projection while composing.\n{detail}"));
    let name = "I7 a rendered heading block flips to source when composing and keeps it until the caret leaves";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "one source block at every step; markers painted; back to rendered once the caret left"
                .to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- I8
    // The spec's candidate-window scenario names an *indented* list item, so the
    // anchor has to follow the block's content edge rather than the page edge.
    let mut failures: Vec<String> = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    let list_item = view.layout().and_then(|layout| {
        layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, typors_core::doc::BlockKind::ListItem { .. }) && !b.spans.is_empty())
            .map(|b| (b.block_idx, b.src.clone(), b.content_left, b.line_height))
    });
    match list_item {
        Some((l_idx, l_src, left, line_height)) => {
            let start = view.doc().snap_floor(l_src.start + 2).max(l_src.start);
            view.set_caret(start);
            view.sync_layout();
            let rope = view.doc().text();
            let revision = view.doc().revision();
            view.replace_and_mark_text_in_range(None, "xie", Some(0..3), window, cx);
            view.sync_layout();
            detail.push_str(&step_line(view, "8 list item compstr", "compstr=xie"));
            let bounds = view
                .selected_text_range(false, window, cx)
                .and_then(|sel| {
                    let composite = view.marked().map(|m| m.splice(&view.doc().text(), 0).0)?;
                    let queried = utf16_to_byte(&composite, sel.range.start);
                    let bounds = view.bounds_for_range(sel.range.clone(), element_bounds, window, cx);
                    let pos = view
                        .layout()
                        .and_then(|layout| composite_pos(layout, view.doc(), queried));
                    Some((bounds, pos))
                });
            match bounds {
                Some((Some(bounds), Some(pos))) => {
                    detail.push_str(&format!(
                        "   list item anchor         block {} content_left={:.1} bounds {} line height {:.1}\n",
                        l_idx,
                        left.as_f32(),
                        fmt_bounds(Some(bounds)),
                        line_height.as_f32()
                    ));
                    if bounds.origin.x.as_f32() < left.as_f32() - 0.5 {
                        failures.push(format!(
                            "the candidate anchor x {:.1} is left of the list item's content edge {:.1}",
                            bounds.origin.x.as_f32(),
                            left.as_f32()
                        ));
                    }
                    if let Some(origin) = pos.span_origin
                        && !close(bounds.origin.x, origin.x, 0.5)
                    {
                        failures.push(format!(
                            "the list item anchor {} is not the fragment at x={:.1}",
                            fmt_bounds(Some(bounds)),
                            origin.x.as_f32()
                        ));
                    }
                    if !close(bounds.size.height, line_height, 0.5) {
                        failures.push(format!(
                            "the list item anchor height {:.1} != line height {:.1}",
                            bounds.size.height.as_f32(),
                            line_height.as_f32()
                        ));
                    }
                }
                _ => failures.push("no candidate anchor inside the list item".to_string()),
            }
            // Cancel, so the list item case leaves no trace either.
            view.replace_text_in_range(None, "", window, cx);
            view.sync_layout();
            if view.doc().text() != rope || view.doc().revision() != revision {
                failures.push("the list item composition changed the buffer".to_string());
            }
        }
        None => skipped = Some("no list item block with painted fragments".to_string()),
    }
    report.push_str(&format!("\n-- I8 the candidate anchor follows a list item's indent.\n{detail}"));
    let name = "I8 the candidate anchor follows an indented block's content edge";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "anchor x sits at the list content edge with the full line height".to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- restore
    let mut failures: Vec<String> = Vec::new();
    let restored_before = view.doc().text();
    if restored_before != original {
        view.apply_edit(0..view.doc().len_bytes(), &original);
    }
    view.sync_layout();
    if view.doc().text() != original {
        failures.push(format!(
            "restoring the document gave {} bytes, expected {}",
            view.doc().text().len(),
            original.len()
        ));
    }
    if view.marked().is_some() {
        failures.push("the probe left a composition behind".to_string());
    }
    report.push_str(&format!(
        "\n-- restore: buffer {} -> {} bytes, {} edits, rope revision {}\n",
        restored_before.len(),
        view.doc().text().len(),
        view.edit_count(),
        view.doc().revision()
    ));
    checks.push((
        "I0 the probe restores the document and leaves no composition behind".to_string(),
        failures.is_empty(),
        if failures.is_empty() {
            format!("{} bytes, {} edits", view.doc().len_bytes(), view.edit_count())
        } else {
            failures.join("; ")
        },
    ));

    ProbeCapture { report, checks }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    // Named imports only: gpui exports a `test` module, and a glob import would
    // shadow the `#[test]` attribute (the expansion recurses into it).
    use super::MarkedText;

    #[test]
    fn composite_mapping_is_the_identity_without_a_composition() {
        let marked = MarkedText::new("", 5, None);
        for byte in 0..12 {
            assert_eq!(marked.to_source(byte), byte);
            assert_eq!(marked.to_composite(byte), byte);
        }
        assert!(marked.is_empty());
    }

    #[test]
    fn splice_lands_on_a_char_boundary_and_maps_back_to_the_anchor() {
        let slice = "这是正文";
        // The anchor is always a char boundary in practice: the view snaps the
        // caret before it records one.
        let marked = MarkedText::new("nihao", 3, Some(0..5));
        let (spliced, local) = marked.splice(slice, 0);
        assert_eq!(local, 3);
        // Byte 3 is the boundary after the first character, so the composition
        // starts between `这` and `是`.
        assert_eq!(spliced, "这nihao是正文");
        assert_eq!(spliced.get(local..local + 5), Some("nihao"));

        // Composite <-> buffer: the composition collapses onto the anchor, and
        // everything after it shifts by the composition length.
        assert_eq!(marked.to_source(0), 0);
        assert_eq!(marked.to_source(3), 3);
        assert_eq!(marked.to_source(8), 3);
        assert_eq!(marked.to_source(11), 6);
        assert_eq!(marked.to_composite(0), 0);
        assert_eq!(marked.to_composite(3), 3);
        assert_eq!(marked.to_composite(6), 11);
        assert_eq!(marked.composite_range(), 3..8);
        assert_eq!(marked.composite_caret(), 8);
    }

    #[test]
    fn splice_never_cuts_a_character_even_with_a_stale_anchor() {
        // Defensive: an anchor that is not a boundary (it cannot happen through
        // the editor, which snaps) still splices on a boundary instead of
        // panicking on the string slice.
        let (spliced, local) = MarkedText::new("x", 4, None).splice("这是", 0);
        assert_eq!(local, 3);
        assert_eq!(spliced, "这x是");
    }

    #[test]
    fn splice_after_the_slice_keeps_the_slice_whole() {
        let marked = MarkedText::new("abc", 100, None);
        let (spliced, local) = marked.splice("尾", 0);
        assert_eq!(local, "尾".len());
        assert_eq!(spliced, "尾abc");
        assert_eq!(marked.composite_caret(), 103);
    }
}
