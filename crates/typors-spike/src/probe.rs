//! Programmatic stand-in for the manual checks of tasks 4.1 / 4.2 / 4.3.
//!
//! Nobody can "click ten times and confirm the caret landed in the right block"
//! from a terminal, so every claim of group 4 becomes an assertion here:
//!
//! * `C1..C5` — caret geometry, point hit-testing and keyboard movement (4.1).
//! * `C6` — the caret block switches to Markdown source and nothing else does (4.2).
//! * `E1..E4` — typing and deleting re-parse, re-lay-out, keep the render
//!   projection and keep the mapping reversible (4.3).
//!
//! The probe drives [`MarkdownView`] — the same entity the window paints — and
//! only through its public editor API (`click`, `move_caret`, `apply_edit`, …).
//! It runs against whatever [`crate::render::Metrics`] the layout already holds:
//! the window text system when a surface exists (the real thing, measured by the
//! same call that paints), or [`crate::render::ApproxMetrics`] when nothing could
//! be painted. The report says which, so a reader never has to guess.

use std::ops::Range;

use gpui_kit::gpui::*;

use crate::doc::BlockKind;
use crate::render::{BlockBox, BlockMode, DocLayout, SOURCE_SPAN};
use crate::view::{CaretMove, MarkdownView};

/// What a probe run produced: a human-readable transcript plus pass/fail checks.
#[derive(Clone)]
pub struct ProbeCapture {
    pub report: String,
    pub checks: Vec<(String, bool, String)>,
}

// ---------------------------------------------------------------- helpers

/// Byte offsets at every char boundary of `text`, including `text.len()`.
fn boundaries(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    out.push(text.len());
    out
}

/// Previous char boundary strictly before `at`.
fn prev_boundary(text: &str, at: usize) -> usize {
    let mut i = at.min(text.len()).saturating_sub(1);
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `text` with `range` replaced — computed independently of the editor, so an
/// edit assertion is not just the editor agreeing with itself.
fn spliced(text: &str, range: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
}

/// A point in the middle of a caret rectangle: what the mouse would report.
fn caret_point(rect: Bounds<Pixels>) -> Point<Pixels> {
    point(
        px(rect.origin.x.as_f32() + 1.),
        px(rect.origin.y.as_f32() + rect.size.height.as_f32() * 0.5),
    )
}

fn format_bounds(rect: Option<Bounds<Pixels>>) -> String {
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

/// A check whose precondition this document does not meet. It is *not* a pass
/// claim: the detail says so, and the check name stays visible.
fn not_exercised(name: &str, why: &str) -> (String, bool, String) {
    (name.to_string(), true, format!("not exercised: {why}"))
}

fn projection_label(mode: BlockMode) -> &'static str {
    match mode {
        BlockMode::Rendered => "rendered",
        BlockMode::Source => "source",
    }
}

fn painted(layout: &DocLayout, block_idx: usize) -> String {
    layout
        .blocks
        .get(block_idx)
        .map(|b| b.spans.iter().map(|s| s.text.as_str()).collect())
        .unwrap_or_default()
}

fn source_block_indices(layout: &DocLayout) -> Vec<usize> {
    layout
        .blocks
        .iter()
        .filter(|b| b.mode == BlockMode::Source)
        .map(|b| b.block_idx)
        .collect()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn describe_caret(view: &MarkdownView) -> String {
    match view.caret_state() {
        Some(state) => {
            let block = view.layout().and_then(|l| l.blocks.get(state.block_idx));
            format!(
                "caret src={} block={} byte={} rect={} | block is {:?}, projected as {}",
                state.src,
                state.block_idx,
                state.display_byte,
                format_bounds(state.bounds),
                block.map(|b| b.kind.clone()),
                block
                    .map(|b| projection_label(b.mode))
                    .unwrap_or("(no block)"),
            )
        }
        None => format!("caret src={:?} (no layout)", view.caret()),
    }
}

fn format_checks(checks: &[(String, bool, String)]) -> String {
    let mut out = String::from("\n-- checks --\n");
    for (name, ok, detail) in checks {
        out.push_str(&format!(
            "  [{}] {name} — {detail}\n",
            if *ok { "ok" } else { "FAIL" }
        ));
    }
    out
}

/// The whole transcript plus checks, for the caller to print and to exit on.
pub fn render_capture(capture: &ProbeCapture) -> bool {
    print!("{}", capture.report);
    print!("{}", format_checks(&capture.checks));
    capture.checks.iter().all(|(_, ok, _)| *ok)
}

fn projection_table(layout: &DocLayout, title: &str) -> String {
    let mut out = format!("\n-- projection after: {title}\n");
    out.push_str("   block  kind                        projection  caret rect         painted\n");
    for block in &layout.blocks {
        let painted: String = block.spans.iter().map(|s| s.text.as_str()).collect();
        let caret = layout
            .caret_bounds(block.block_idx, 0)
            .map(|b| format_bounds(Some(b)))
            .unwrap_or_else(|| "(none)".to_string());
        out.push_str(&format!(
            "   {:>5}  {:<27} {:<11} {:<18} {:?}\n",
            block.block_idx,
            format!("{:?}", block.kind),
            projection_label(block.mode),
            caret,
            truncate(&painted, 56)
        ));
    }
    out
}

// ---------------------------------------------------------------- 4.1 cursor

/// A `--dump-cursor` run: hit-testing, caret geometry, projection switch and
/// keyboard movement.
pub fn cursor_probe(view: &mut MarkdownView, target: Option<(usize, usize)>) -> ProbeCapture {
    view.ensure_layout();
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut report = String::new();
    let doc_text = view.doc().text();
    let (metrics_label, block_count, fragment_count) = match view.layout() {
        Some(layout) => (
            layout.metrics_label,
            layout.blocks.len(),
            layout.blocks.iter().map(|b| b.spans.len()).sum::<usize>(),
        ),
        None => ("<no layout>", 0, 0),
    };
    report.push_str(&format!(
        "# cursor probe: {block_count} blocks, {fragment_count} painted fragments, measured by {metrics_label}\n\
         # buffer: {} bytes, {} blocks, revision {}\n",
        doc_text.len(),
        view.doc().blocks().len(),
        view.doc().revision(),
    ));

    // ---------------------------------------------------------------- C1
    // No caret at all: everything rendered. caret rect -> point -> hit -> caret
    // rect must round-trip on every fragment.
    view.clear_caret();
    view.sync_layout();
    let mut failures = Vec::new();
    let mut probes = 0usize;
    check_rendered_roundtrip(view, &mut failures, &mut probes);
    checks.push((
        "C1 all-rendered caret rect -> hit -> caret rect round-trips".to_string(),
        failures.is_empty(),
        if failures.is_empty() {
            format!("{probes} probes over {block_count} blocks")
        } else {
            failures.join("; ")
        },
    ));

    // ---------------------------------------------------------------- C2
    // Every block as the source block: display byte -> caret rect -> hit ->
    // display byte must round-trip exactly. This is the projection the caret
    // actually lives in, and the one whose geometry the `## ` prefix changes.
    let mut failures = Vec::new();
    let mut probes = 0usize;
    let mut silent = Vec::new();
    let mut unpainted = 0usize;
    for block_idx in 0..block_count {
        check_source_roundtrip(
            view,
            block_idx,
            &mut failures,
            &mut probes,
            &mut silent,
            &mut unpainted,
        );
    }
    checks.push((
        "C2 each block as source: display byte -> caret rect -> hit -> display byte".to_string(),
        failures.is_empty(),
        if failures.is_empty() {
            format!(
                "{probes} probes over {block_count} blocks; {unpainted} unpainted offsets (trailing newlines) skipped{}",
                if silent.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; {} blocks have no fragments (blank lines), e.g. {:?}",
                        silent.len(),
                        &silent[..silent.len().min(6)]
                    )
                }
            )
        } else {
            failures.join("; ")
        },
    ));

    // ---------------------------------------------------------------- C3
    view.clear_caret();
    view.sync_layout();
    let mut failures = Vec::new();
    let mut probes = 0usize;
    check_boundaries(view, &mut failures, &mut probes, &mut report);
    checks.push((
        "C3 block edges: first/last block, every block's start and end, blank lines".to_string(),
        failures.is_empty(),
        if failures.is_empty() {
            format!("{probes} probes")
        } else {
            failures.join("; ")
        },
    ));

    // ---------------------------------------------------------------- C4
    let mut failures = Vec::new();
    let mut sequence = Vec::new();
    let skipped = check_keyboard(view, &mut failures, &mut sequence);
    let name = "C4 arrow/Home/End movement: (block, byte) sequence as expected, caret never stuck";
    checks.push(match (skipped, failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            format!("{} moves", sequence.len()),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });
    report.push_str("\n-- C4 movement sequence --\n");
    for step in &sequence {
        report.push_str(&format!("   {step}\n"));
    }

    // ---------------------------------------------------------------- C5
    let mut failures = Vec::new();
    let mut placed = String::new();
    if let Some((block_idx, byte)) = target {
        match view.doc().blocks().get(block_idx) {
            Some(block) => {
                let src = view.doc().snap_floor(block.src.start + byte.min(block.src.len()));
                view.set_caret(src);
                view.sync_layout();
                placed = describe_caret(view);
                match view.caret_block() {
                    Some(actual) if actual == block_idx => {}
                    other => failures.push(format!(
                        "asked for block {block_idx}:{byte} (src {src}), caret block is {other:?}"
                    )),
                }
            }
            None => failures.push(format!(
                "block {block_idx} does not exist ({} blocks)",
                view.doc().blocks().len()
            )),
        }
    }
    checks.push((
        "C5 --dump-cursor <block>:<byte> lands on the requested block".to_string(),
        failures.is_empty(),
        if target.is_none() {
            "no target given (pass <block>:<byte> to name a spot)".to_string()
        } else if failures.is_empty() {
            placed.clone()
        } else {
            failures.join("; ")
        },
    ));

    // ---------------------------------------------------------------- C6 (4.2)
    let mut failures = Vec::new();
    let skipped = check_projection_switch(view, &mut failures, &mut report);
    let name = "C6 caret block paints its Markdown source, every other block stays rendered";
    checks.push(match (skipped, failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "switched into the heading and back out, both directions asserted".to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // Layout-level checks (A1..A5) in the projection the caret is in.
    view.sync_layout();
    if let Some(layout) = view.layout() {
        for (name, ok, detail) in layout.checks(view.doc()) {
            checks.push((format!("[caret layout] {name}"), ok, detail));
        }
    }

    // H1 datum for group 4: what the source projection of one block costs on top
    // of the render projection (the layout copies the buffer once per frame to
    // slice the caret block).
    if let Some(block_idx) = view.caret_block() {
        let rounds = 5;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(view.layout_for_source(Some(block_idx)));
        }
        let per_layout = started.elapsed().as_secs_f64() * 1000. / rounds as f64;
        report.push_str(&format!(
            "\n-- H1: relayout with block {block_idx} in the source projection: {per_layout:.3} ms (mean of {rounds}, {} blocks, {} bytes)\n",
            block_count,
            doc_text.len()
        ));
    }

    ProbeCapture { report, checks }
}

fn check_rendered_roundtrip(
    view: &MarkdownView,
    failures: &mut Vec<String>,
    probes: &mut usize,
) {
    let Some(layout) = view.layout() else {
        failures.push("no layout".to_string());
        return;
    };
    for block in &layout.blocks {
        if block.mode != BlockMode::Rendered {
            continue;
        }
        for span in &block.spans {
            for local in boundaries(&span.text) {
                *probes += 1;
                let display_byte = span.render.start + local;
                let Some((x, y, line_height)) = layout.caret_x(block.block_idx, display_byte) else {
                    failures.push(format!(
                        "block {} byte {display_byte}: no caret x",
                        block.block_idx
                    ));
                    continue;
                };
                let probe = point(
                    px(x.as_f32() + 1.),
                    px(y.as_f32() + line_height.as_f32() * 0.5),
                );
                let Some(hit) = layout.hit(probe) else {
                    failures.push(format!(
                        "block {} byte {display_byte}: hit at ({:.1},{:.1}) is None",
                        block.block_idx,
                        probe.x.as_f32(),
                        probe.y.as_f32()
                    ));
                    continue;
                };
                if hit.block_idx != block.block_idx {
                    failures.push(format!(
                        "block {} byte {display_byte}: hit reported block {}",
                        block.block_idx, hit.block_idx
                    ));
                    continue;
                }
                match layout.caret_x(hit.block_idx, hit.render_byte) {
                    Some((x2, _, _)) if (x2.as_f32() - x.as_f32()).abs() <= 1.5 => {}
                    other => failures.push(format!(
                        "block {} byte {display_byte}: x {:.1} -> hit byte {} -> {other:?}",
                        block.block_idx, x.as_f32(), hit.render_byte
                    )),
                }
            }
        }
    }
}

fn check_source_roundtrip(
    view: &MarkdownView,
    block_idx: usize,
    failures: &mut Vec<String>,
    probes: &mut usize,
    silent: &mut Vec<usize>,
    unpainted: &mut usize,
) {
    let Some(layout) = view.layout_for_source(Some(block_idx)) else {
        failures.push("no layout".to_string());
        return;
    };
    let Some(block) = layout.blocks.get(block_idx) else {
        failures.push(format!("block {block_idx} missing from the layout"));
        return;
    };
    if block.mode != BlockMode::Source {
        failures.push(format!("block {block_idx} is not projected as source"));
        return;
    }
    let text = block.display_text().to_string();
    if block.spans.is_empty() {
        silent.push(block_idx);
        return;
    }
    let base = block.src.start;
    for local in boundaries(&text) {
        // A byte with no painted position cannot be hit: the block's trailing
        // newline is such a byte (it terminates the line instead of occupying
        // one). The caret at that offset paints at the end of the line, which is
        // the *previous* boundary, and `hit` can only report painted positions.
        if !block
            .spans
            .iter()
            .any(|s| s.render.start <= local && local <= s.render.end)
        {
            *unpainted += 1;
            continue;
        }
        *probes += 1;
        let expected = base + local;
        let Some(rect) = layout.caret_bounds(block_idx, local) else {
            failures.push(format!("block {block_idx} byte {local}: no caret rect"));
            continue;
        };
        let probe = caret_point(rect);
        let Some(hit) = layout.hit(probe) else {
            failures.push(format!(
                "block {block_idx} byte {local}: hit at ({:.1},{:.1}) is None",
                probe.x.as_f32(),
                probe.y.as_f32()
            ));
            continue;
        };
        if hit.block_idx != block_idx {
            failures.push(format!(
                "block {block_idx} byte {local}: hit reported block {}",
                hit.block_idx
            ));
            continue;
        }
        match layout.src_for_display_byte(view.doc(), hit.block_idx, hit.render_byte) {
            Some(src) if src == expected => {}
            other => failures.push(format!(
                "block {block_idx} byte {local} (src {expected}): hit byte {} -> {other:?}",
                hit.render_byte
            )),
        }
    }
}

fn check_boundaries(
    view: &MarkdownView,
    failures: &mut Vec<String>,
    probes: &mut usize,
    report: &mut String,
) {
    let Some(layout) = view.layout() else {
        failures.push("no layout".to_string());
        return;
    };
    let doc = view.doc();

    // Every block's own vertical band resolves to itself, in order.
    let mut last = 0usize;
    for block in &layout.blocks {
        if block.bounds.size.height.as_f32() <= 0. {
            continue;
        }
        for fraction in [0.05f32, 0.5, 0.95] {
            *probes += 1;
            let y = px(
                block.bounds.origin.y.as_f32()
                    + block.bounds.size.height.as_f32() * fraction,
            );
            match layout.block_at_y(y) {
                Some(idx) if idx == block.block_idx => {}
                other => failures.push(format!(
                    "block {} y={:.1}: block_at_y gave {other:?}",
                    block.block_idx,
                    y.as_f32()
                )),
            }
        }
        if block.block_idx < last {
            failures.push(format!(
                "block_at_y walked backwards: {} after {last}",
                block.block_idx
            ));
        }
        last = block.block_idx;
    }

    // Above the first block and below the last one clamp instead of failing.
    if let (Some(first), Some(last_block)) = (layout.blocks.first(), layout.blocks.last()) {
        *probes += 2;
        let above = point(px(first.bounds.origin.x.as_f32() + 1.), px(-10.));
        match layout.hit(above) {
            Some(hit) if hit.block_idx == first.block_idx => {}
            other => failures.push(format!("hit above the document: {other:?}")),
        }
        let below = point(
            px(last_block.bounds.origin.x.as_f32() + 1.),
            px(last_block.bounds.bottom().as_f32() + 500.),
        );
        match layout.hit(below) {
            Some(hit) if hit.block_idx == last_block.block_idx => {}
            other => failures.push(format!("hit below the document: {other:?}")),
        }
    }

    // Every block's first and last byte maps to a char boundary and back.
    for block in &layout.blocks {
        let display_len = block.display_text().len();
        for (label, byte) in [("start", 0usize), ("end", display_len)] {
            *probes += 1;
            let Some(src) = layout.src_for_display_byte(doc, block.block_idx, byte) else {
                failures.push(format!(
                    "block {} {label}: no source offset",
                    block.block_idx
                ));
                continue;
            };
            if !doc.is_char_boundary(src) {
                failures.push(format!(
                    "block {} {label}: source offset {src} is not a char boundary",
                    block.block_idx
                ));
            }
            if layout.display_byte_for_src(doc, src).is_none() {
                failures.push(format!(
                    "block {} {label}: source offset {src} does not map back",
                    block.block_idx
                ));
            }
        }
    }

    // A blank line is clickable: a `Gap` with no fragments still owns a line box.
    let mut gaps = 0usize;
    for block in &layout.blocks {
        if block.kind != BlockKind::Gap || block.bounds.size.height.as_f32() <= 0. {
            continue;
        }
        gaps += 1;
        *probes += 1;
        let mid = px(block.bounds.origin.y.as_f32() + block.bounds.size.height.as_f32() * 0.5);
        match layout.block_at_y(mid) {
            Some(idx) if idx == block.block_idx => {}
            other => failures.push(format!(
                "blank block {}: block_at_y gave {other:?}",
                block.block_idx
            )),
        }
        let p = point(px(block.bounds.origin.x.as_f32() + 2.), mid);
        match layout.hit(p) {
            Some(hit) => {
                if hit.render_byte != 0 {
                    failures.push(format!(
                        "blank block {}: hit byte {} != 0",
                        block.block_idx, hit.render_byte
                    ));
                }
                match layout.src_for_display_byte(doc, hit.block_idx, hit.render_byte) {
                    Some(src) if src == block.src.start => {}
                    other => failures.push(format!(
                        "blank block {}: source {other:?} != {}",
                        block.block_idx, block.src.start
                    )),
                }
            }
            None => failures.push(format!("blank block {}: hit is None", block.block_idx)),
        }
    }
    report.push_str(&format!(
        "-- C3: {probes} boundary probes, {gaps} blank-line blocks clickable\n"
    ));
}

fn check_keyboard(
    view: &mut MarkdownView,
    failures: &mut Vec<String>,
    sequence: &mut Vec<String>,
) -> Option<String> {
    let doc_text = view.doc().text();
    let Some(layout) = view.layout().cloned() else {
        return Some("no layout".to_string());
    };
    let Some((p_idx, p_src)) = layout
        .blocks
        .iter()
        .find(|b| b.kind == BlockKind::Paragraph && b.spans.len() >= 3)
        .map(|b| (b.block_idx, b.src.clone()))
    else {
        return Some("no paragraph block with three or more styled fragments".to_string());
    };

    // Right walks one char boundary at a time, inside the paragraph.
    view.set_caret(p_src.start + 3);
    view.sync_layout();
    let start = view.caret().unwrap_or(0);
    sequence.push(format!("start: {}", describe_caret(view)));
    for _ in 0..3 {
        if !view.move_caret(CaretMove::Right) {
            failures.push("right refused to move".to_string());
            return None;
        }
        view.sync_layout();
        sequence.push(format!("right: {}", describe_caret(view)));
    }
    let expected = advance_chars(&doc_text, start, 3);
    match view.caret() {
        Some(caret) if caret == expected => {}
        other => failures.push(format!(
            "3x right from {start}: expected {expected}, got {other:?}"
        )),
    }
    for _ in 0..3 {
        view.move_caret(CaretMove::Left);
        view.sync_layout();
    }
    match view.caret() {
        Some(caret) if caret == start => {}
        other => failures.push(format!(
            "3x right then 3x left: expected {start}, got {other:?}"
        )),
    }

    // Left at the very start of the buffer is a no-op, not an underflow, and the
    // caret stays in the first block.
    view.move_caret(CaretMove::DocStart);
    view.sync_layout();
    let at_zero = view.caret();
    view.move_caret(CaretMove::Left);
    view.sync_layout();
    match (at_zero, view.caret()) {
        (Some(0), Some(0)) => {}
        other => failures.push(format!("left at the document start: {other:?}")),
    }
    sequence.push(format!("doc start: {}", describe_caret(view)));

    // Crossing a block boundary: right at the end of a paragraph's last char
    // moves into the next block, and left comes back.
    view.set_caret(p_src.end - 1);
    view.sync_layout();
    let at_last_char = view.caret();
    view.move_caret(CaretMove::Right);
    view.sync_layout();
    let crossed = view.caret_block();
    if crossed == Some(p_idx) {
        failures.push("right at the paragraph end did not leave the block".to_string());
    }
    view.move_caret(CaretMove::Left);
    view.sync_layout();
    match view.caret() {
        Some(caret) if Some(caret) == at_last_char => {}
        other => failures.push(format!(
            "crossing back: expected {at_last_char:?}, got {other:?}"
        )),
    }
    sequence.push(format!("crossed right and back: {}", describe_caret(view)));

    // Home/End stay inside the block and honour its trailing newline.
    view.set_caret(p_src.start + 3);
    view.sync_layout();
    view.move_caret(CaretMove::LineEnd);
    view.sync_layout();
    let slice = doc_text.get(p_src.clone()).unwrap_or_default();
    let trimmed = p_src.start + slice.trim_end_matches('\n').len();
    match view.caret() {
        Some(caret) if caret == trimmed => {}
        other => failures.push(format!(
            "line end of block {p_idx}: expected {trimmed}, got {other:?}"
        )),
    }
    sequence.push(format!("line end: {}", describe_caret(view)));
    view.move_caret(CaretMove::LineStart);
    view.sync_layout();
    match view.caret() {
        Some(caret) if caret == p_src.start => {}
        other => failures.push(format!(
            "line start of block {p_idx}: expected {}, got {other:?}",
            p_src.start
        )),
    }
    sequence.push(format!("line start: {}", describe_caret(view)));

    // Down goes to a lower line, up returns to the same line.
    view.set_caret(p_src.start + 3);
    view.sync_layout();
    let before = view.caret_state();
    view.move_caret(CaretMove::Down);
    view.sync_layout();
    let down = view.caret_state();
    match (before, down) {
        (Some(before), Some(down)) => {
            let y_before = before.bounds.map_or(0., |b| b.origin.y.as_f32());
            let y_down = down.bounds.map_or(0., |b| b.origin.y.as_f32());
            if y_down <= y_before {
                failures.push(format!(
                    "down did not reach a lower line: {y_before:.1} -> {y_down:.1}"
                ));
            }
            sequence.push(format!("down: {}", describe_caret(view)));
            view.move_caret(CaretMove::Up);
            view.sync_layout();
            let y_up = view
                .caret_state()
                .and_then(|s| s.bounds)
                .map_or(-1., |b| b.origin.y.as_f32());
            if (y_up - y_before).abs() > 0.5 {
                failures.push(format!(
                    "down then up changed the line: {y_before:.1} -> {y_up:.1}"
                ));
            }
            sequence.push(format!("up again: {}", describe_caret(view)));
        }
        _ => failures.push("vertical movement lost the caret".to_string()),
    }

    // Vertical movement at the document edges is refused, not clamped into a
    // neighbouring block.
    view.set_caret(view.doc().len_bytes());
    view.sync_layout();
    let at_end = view.caret();
    if view.move_caret(CaretMove::Down) {
        failures.push("down at the document end moved the caret".to_string());
    }
    if view.caret() != at_end {
        failures.push(format!(
            "down at the document end changed the caret: {at_end:?} -> {:?}",
            view.caret()
        ));
    }
    view.set_caret(0);
    view.sync_layout();
    if view.move_caret(CaretMove::Up) {
        failures.push("up at the document start moved the caret".to_string());
    }

    // A run of downs from the top always walks down: the caret line strictly
    // increases and never gets stuck. (Capped, and only asserted to the very end
    // on documents small enough that one keystroke per line is a sane probe.)
    let block_count = view.doc().blocks().len();
    let cap = (block_count * 2 + 16).min(600);
    view.set_caret(0);
    view.sync_layout();
    let mut moves = 0usize;
    let mut last = view.caret();
    let mut prev_y = view
        .caret_state()
        .and_then(|s| s.bounds)
        .map_or(f32::MIN, |b| b.origin.y.as_f32());
    let mut reached_end = false;
    while moves < cap {
        let moved = view.move_caret(CaretMove::Down);
        view.sync_layout();
        moves += 1;
        if !moved && view.caret() == last {
            reached_end = true;
            break;
        }
        last = view.caret();
        let y = view
            .caret_state()
            .and_then(|s| s.bounds)
            .map_or(prev_y, |b| b.origin.y.as_f32());
        if y <= prev_y {
            failures.push(format!(
                "down #{moves} did not go down: y {prev_y:.1} -> {y:.1}"
            ));
            break;
        }
        prev_y = y;
    }
    let last_block = block_count.saturating_sub(1);
    if reached_end {
        match view.caret_block() {
            Some(idx) if idx == last_block => {}
            other => failures.push(format!(
                "{moves} downs from the top ended in block {other:?}, expected {last_block}"
            )),
        }
    }
    sequence.push(format!("after {moves} downs: {}", describe_caret(view)));

    view.move_caret(CaretMove::DocStart);
    view.sync_layout();
    if view.caret() != Some(0) {
        failures.push(format!("document start gave {:?}", view.caret()));
    }
    view.move_caret(CaretMove::DocEnd);
    view.sync_layout();
    if view.caret() != Some(view.doc().len_bytes()) {
        failures.push(format!(
            "document end gave {:?}, expected {}",
            view.caret(),
            view.doc().len_bytes()
        ));
    }
    sequence.push(format!("doc end: {}", describe_caret(view)));
    None
}

fn advance_chars(text: &str, from: usize, count: usize) -> usize {
    let mut at = from;
    for _ in 0..count {
        if at >= text.len() {
            return text.len();
        }
        at += 1;
        while at < text.len() && !text.is_char_boundary(at) {
            at += 1;
        }
    }
    at
}

// ---------------------------------------------------------------- 4.2 switch

fn check_projection_switch(
    view: &mut MarkdownView,
    failures: &mut Vec<String>,
    report: &mut String,
) -> Option<String> {
    let Some(layout) = view.layout().cloned() else {
        return Some("no layout".to_string());
    };
    // Prefer the spec's own scenario (a level-2 heading), then any heading; the
    // block to move the caret out to can be of any kind.
    let heading = layout
        .blocks
        .iter()
        .find(|b| matches!(b.kind, BlockKind::Heading { level } if level == 2))
        .or_else(|| {
            layout
                .blocks
                .iter()
                .find(|b| matches!(b.kind, BlockKind::Heading { .. }))
        });
    let Some((h_idx, h_src)) = heading.map(|b| (b.block_idx, b.src.clone())) else {
        return Some("no heading block in this document".to_string());
    };
    let after_heading = |b: &&BlockBox| b.block_idx != h_idx && b.src.start >= h_src.end;
    let has_text = |b: &&BlockBox| !b.spans.is_empty();
    let target = layout
        .blocks
        .iter()
        .filter(has_text)
        .find(|b| after_heading(b) && b.kind == BlockKind::Paragraph)
        .or_else(|| layout.blocks.iter().filter(has_text).find(after_heading))
        .or_else(|| layout.blocks.iter().filter(has_text).find(|b| b.block_idx != h_idx))
        .or_else(|| layout.blocks.iter().find(|b| b.block_idx != h_idx));
    let Some((p_idx, p_src)) = target.map(|b| (b.block_idx, b.src.clone())) else {
        return Some("no other block to move the caret out to".to_string());
    };

    // Into the heading: the spec's own scenario is "## " appears and the other
    // headings still show none. (A block owns the caret at its own start offset,
    // so no further movement is needed to be "inside".)
    view.set_caret(h_src.start);
    view.sync_layout();
    let Some(inside) = view.layout().cloned() else {
        return Some("no layout after moving into the heading".to_string());
    };
    report.push_str(&projection_table(
        &inside,
        &format!("caret inside the heading (block {h_idx})"),
    ));
    if inside.block_mode(h_idx) != Some(BlockMode::Source) {
        failures.push(format!("block {h_idx} did not switch to source"));
    }
    let in_heading = painted(&inside, h_idx);
    if !in_heading.starts_with("## ") {
        failures.push(format!(
            "caret heading painted {in_heading:?}, expected a leading \"## \""
        ));
    }
    if source_block_indices(&inside) != vec![h_idx] {
        failures.push(format!(
            "source blocks {:?}, expected only [{h_idx}]",
            source_block_indices(&inside)
        ));
    }
    for block in &inside.blocks {
        if block.block_idx == h_idx {
            continue;
        }
        if block.mode != BlockMode::Rendered {
            failures.push(format!("block {} is not rendered", block.block_idx));
        }
        if matches!(block.kind, BlockKind::Heading { .. })
            && painted(&inside, block.block_idx).contains('#')
        {
            failures.push(format!(
                "sibling heading {} paints '#': {:?}",
                block.block_idx,
                painted(&inside, block.block_idx)
            ));
        }
    }

    // Out again: the heading restores, its `## ` disappears, and the paragraph
    // takes over as the single source block.
    view.set_caret(p_src.start);
    view.sync_layout();
    let Some(outside) = view.layout().cloned() else {
        return Some("no layout after moving out of the heading".to_string());
    };
    report.push_str(&projection_table(
        &outside,
        &format!("caret moved out to block {p_idx}"),
    ));
    if outside.block_mode(h_idx) != Some(BlockMode::Rendered) {
        failures.push(format!("block {h_idx} stayed in source after the caret left"));
    }
    let restored = painted(&outside, h_idx);
    let expected = outside
        .blocks
        .get(h_idx)
        .map(|b| b.render.clone())
        .unwrap_or_default();
    if restored != expected {
        failures.push(format!(
            "restored heading painted {restored:?} != its render text {expected:?}"
        ));
    }
    if source_block_indices(&outside) != vec![p_idx] {
        failures.push(format!(
            "source blocks {:?}, expected only [{p_idx}]",
            source_block_indices(&outside)
        ));
    }
    report.push_str(&format!(
        "\n-- switching evidence: block {h_idx} painted {in_heading:?} with the caret inside, {restored:?} after the caret left\n"
    ));
    None
}

// ---------------------------------------------------------------- 4.3 editing

/// A `--dump-edit` run: typing and deleting through
/// [`MarkdownView::apply_edit`], asserting text, caret and both projections.
pub fn edit_probe(view: &mut MarkdownView) -> ProbeCapture {
    view.ensure_layout();
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut report = String::new();
    let original = view.doc().text();
    let original_blocks = view.doc().blocks().len();
    report.push_str(&format!(
        "# edit probe: {} bytes, {} blocks, measured by {}\n",
        original.len(),
        original_blocks,
        view.layout()
            .map(|l| l.metrics_label)
            .unwrap_or("<no layout>")
    ));

    // ---------------------------------------------------------------- E1
    // Type into a styled paragraph.
    let mut failures = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    let paragraph = view.layout().and_then(|layout| {
        layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Paragraph && b.spans.len() >= 3)
            .map(|b| (b.block_idx, b.src.clone()))
    });
    match paragraph {
        Some((p_idx, p_src)) => {
            view.set_caret(p_src.start + 3);
            view.sync_layout();
            let at = view.caret().unwrap_or(p_src.start);
            view.insert_text("X");
            view.sync_layout();
            let expected = spliced(&original, at..at, "X");
            if view.doc().text() != expected {
                failures.push("typing: buffer is not the expected splice".to_string());
            }
            if view.caret() != Some(at + 1) {
                failures.push(format!(
                    "typing: caret {:?} != {}",
                    view.caret(),
                    at + 1
                ));
            }
            if view.caret_block() != Some(p_idx) {
                failures.push(format!(
                    "typing: caret block {:?} != {p_idx}",
                    view.caret_block()
                ));
            }
            detail.push_str(&projection_invariants(
                view,
                "typed X into a paragraph",
                &mut failures,
            ));
            // And back out again, so the rest of the probe starts from the
            // original buffer.
            view.delete_backward();
            view.sync_layout();
            detail.push_str(&projection_invariants(
                view,
                "deleted the X again",
                &mut failures,
            ));
            if view.doc().text() != original {
                failures.push("typing then deleting did not restore the buffer".to_string());
            }
        }
        None => skipped = Some("no paragraph block with styled fragments".to_string()),
    }
    report.push_str(&format!("\n-- E1 typing.\n{detail}"));
    let name = "E1 typing in a paragraph: text, caret, source block, rendered siblings";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "inserted 1 char, caret followed, buffer restored".to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- E2
    // Inside a heading line: delete a char, then type it back.
    let mut failures = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    let heading = view.layout().and_then(|layout| {
        layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Heading { level } if level == 2))
            .map(|b| (b.block_idx, b.src.clone()))
    });
    match heading {
        Some((h_idx, h_src)) => {
            // Just after `## ` plus one title character (a char boundary).
            let mut at = h_src.start + 6;
            while at < h_src.end && !view.doc().is_char_boundary(at) {
                at += 1;
            }
            view.set_caret(at);
            view.sync_layout();
            let at = view.caret().unwrap_or(at);
            let text_before = view.doc().text();
            let removed_start = prev_boundary(&text_before, at);
            let removed = text_before[removed_start..at].to_string();
            view.delete_backward();
            view.sync_layout();
            let expected = spliced(&text_before, removed_start..at, "");
            if view.doc().text() != expected {
                failures.push("heading delete: buffer is not the expected splice".to_string());
            }
            if view.caret() != Some(removed_start) {
                failures.push(format!(
                    "heading delete: caret {:?} != {removed_start}",
                    view.caret()
                ));
            }
            match view
                .caret_block()
                .and_then(|idx| view.doc().blocks().get(idx))
                .map(|b| b.kind.clone())
            {
                Some(BlockKind::Heading { level: 2 }) => {}
                other => failures.push(format!("block after deleting in the heading is {other:?}")),
            }
            let still_heading = view.layout().map(|l| painted(l, h_idx)).unwrap_or_default();
            if !still_heading.starts_with("## ") {
                failures.push(format!(
                    "heading after delete painted {still_heading:?}, markers lost"
                ));
            }
            detail.push_str(&projection_invariants(
                view,
                &format!("deleted {removed:?} inside the heading"),
                &mut failures,
            ));
            view.insert_text(&removed);
            view.sync_layout();
            detail.push_str(&projection_invariants(
                view,
                "typed the character back",
                &mut failures,
            ));
            if view.doc().text() != original {
                failures.push("heading delete + retype did not restore the buffer".to_string());
            }
        }
        None => skipped = Some("no level-2 heading block".to_string()),
    }
    report.push_str(&format!("\n-- E2 heading delete and retype.\n{detail}"));
    let name = "E2 delete and retype inside a heading line: markers kept, buffer restored";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            "the block stayed a level-2 heading with its '## ' prefix".to_string(),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- E3
    // Block edges: backspace at a block start merges it into the previous block.
    let mut failures = Vec::new();
    let mut detail = String::new();
    let mut skipped: Option<String> = None;
    let blocks_before = view.doc().blocks().len();
    let edge = view.layout().and_then(|layout| {
        layout
            .blocks
            .iter()
            .skip(1)
            .find(|b| b.kind == BlockKind::Paragraph && !b.spans.is_empty())
            .map(|b| (b.block_idx, b.src.clone()))
    });
    match edge {
        Some((e_idx, e_src)) => {
            view.set_caret(e_src.start);
            view.sync_layout();
            let at_start = view.caret().unwrap_or(e_src.start);
            view.delete_backward();
            view.sync_layout();
            detail.push_str(&format!(
                "   backspace at block {e_idx} start (src {at_start}): {} blocks -> {}, caret {:?}\n",
                blocks_before,
                view.doc().blocks().len(),
                view.caret()
            ));
            if view.doc().blocks().len() != blocks_before - 1 {
                failures.push(format!(
                    "backspace at a block start gave {} blocks, expected {}",
                    view.doc().blocks().len(),
                    blocks_before - 1
                ));
            }
            detail.push_str(&projection_invariants(
                view,
                "backspaced at the block start",
                &mut failures,
            ));
            view.insert_text("\n");
            view.sync_layout();
            detail.push_str(&projection_invariants(
                view,
                "typed the newline back",
                &mut failures,
            ));
            if view.doc().blocks().len() != blocks_before {
                failures.push(format!(
                    "typing the newline back gave {} blocks, expected {blocks_before}",
                    view.doc().blocks().len()
                ));
            }
        }
        None => skipped = Some("no paragraph block after the first block".to_string()),
    }
    report.push_str(&format!("\n-- E3 block edges.\n{detail}"));
    let name = "E3 backspace at a block start merges blocks, the newline splits them again";
    checks.push(match (skipped.take(), failures.is_empty()) {
        (Some(why), _) => not_exercised(name, &why),
        (None, true) => (
            name.to_string(),
            true,
            format!(
                "blocks {} -> {} -> {}",
                blocks_before,
                blocks_before - 1,
                view.doc().blocks().len()
            ),
        ),
        (None, false) => (name.to_string(), false, failures.join("; ")),
    });

    // ---------------------------------------------------------------- E4
    let mut failures = Vec::new();
    view.sync_layout();
    let restored = view.doc().text();
    if restored != original {
        failures.push(format!(
            "buffer after the edit round-trip is {} bytes, expected {}",
            restored.len(),
            original.len()
        ));
    }
    if view.doc().blocks().len() != original_blocks {
        failures.push(format!(
            "blocks after the round-trip: {} != {original_blocks}",
            view.doc().blocks().len()
        ));
    }
    let mut detail = projection_invariants(view, "after the whole edit sequence", &mut failures);
    let layout_checks = view
        .layout()
        .map(|layout| layout.checks(view.doc()))
        .unwrap_or_default();
    for (name, ok, info) in &layout_checks {
        if !ok {
            failures.push(format!("{name}: {info}"));
        }
    }
    detail.push_str(&format!(
        "   {} edits, {} layout checks (A1..A5) all evaluated\n",
        view.edit_count(),
        layout_checks.len()
    ));
    report.push_str(&format!("\n-- E4 round trip.\n{detail}"));
    checks.push((
        "E4 the edit sequence round-trips: same buffer, same blocks, clean render projection"
            .to_string(),
        failures.is_empty(),
        if failures.is_empty() {
            format!(
                "{} edits, {} layout checks green",
                view.edit_count(),
                layout_checks.len()
            )
        } else {
            failures.join("; ")
        },
    ));

    if let Some(edit) = view.last_edit() {
        report.push_str(&format!(
            "\n-- last edit record (group 5's undo unit): src={:?} removed={:?} inserted={:?} caret {:?} -> {:?} revision {}\n",
            edit.src, edit.removed, edit.inserted, edit.caret_before, edit.caret_after, edit.revision
        ));
    }

    ProbeCapture { report, checks }
}

/// The invariants that must hold after *every* edit: exactly one source block,
/// it holds the caret, its painted text is the buffer slice, every other block is
/// rendered from `Block::render`, and display <-> source round-trips everywhere.
///
/// `pub(crate)` so group 5's `--dump-select` can reuse it verbatim rather than
/// keeping a second, weaker copy: undo/redo must not change these invariants.
pub(crate) fn projection_invariants(
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
        (Some(block), [only]) if block == *only => {}
        // No caret: the document must be fully rendered.
        (None, []) => {}
        (Some(block), other) => failures.push(format!(
            "{what}: caret in block {block}, source blocks {other:?}"
        )),
        (None, other) => failures.push(format!(
            "{what}: no caret, yet blocks {other:?} are projected as source"
        )),
    }
    let doc_text = view.doc().text();
    for block in &layout.blocks {
        let painted: String = block.spans.iter().map(|s| s.text.as_str()).collect();
        let displayed: String = block.display_text().chars().filter(|c| *c != '\n').collect();
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
            // starts there. What must hold — because the caret *is* a source
            // offset — is that the round trip `src -> (block, byte) -> src` is
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

// ---------------------------------------------------------------- newlines

/// The keystroke the Windows platform produces for `VK_RETURN`.
///
/// `parse_immutable` maps `VK_RETURN` to `"enter"` and `process_key` filters the
/// `ToUnicode` result through `!c.is_control()`, so the `\r` Windows would give
/// for that key never reaches the keystroke — `key_char` is `None`.
/// (`gpui-pre-windows/events.rs:1546`, `process_key`.)
pub fn enter_keystroke() -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: "enter".to_string(),
        key_char: None,
    }
}

/// What one press of Enter left behind, in the terms the bug is about.
#[derive(Clone, Debug)]
struct NewlineStep {
    len_bytes: usize,
    caret: Option<usize>,
    anchor: Option<usize>,
    selection: Option<Range<usize>>,
    revision: u64,
    trigger: Option<&'static str>,
    inserted: Option<String>,
    /// The caret block after the edit: index, kind, height, line height.
    block: Option<(usize, String, f32, f32)>,
    /// Consecutive `\n` immediately before the caret (the run an Enter extends).
    trailing_newlines: usize,
    /// Escaped text around the caret, for the transcript.
    window: String,
}

fn newline_step(view: &mut MarkdownView) -> NewlineStep {
    view.sync_layout();
    let doc_text = view.doc().text();
    let caret = view.caret();
    let trailing_newlines = caret
        .map(|at| {
            doc_text[..at.min(doc_text.len())]
                .chars()
                .rev()
                .take_while(|c| *c == '\n')
                .count()
        })
        .unwrap_or(0);
    // A few characters either side of the caret, taken by *character* count so
    // the slice can never land inside a multi-byte character.
    let window = match caret {
        Some(at) => {
            let at = at.min(doc_text.len());
            let before = doc_text[..at].chars().count();
            let from = doc_text
                .char_indices()
                .nth(before.saturating_sub(8))
                .map(|(i, _)| i)
                .unwrap_or(0);
            let to = doc_text
                .char_indices()
                .nth(before + 8)
                .map(|(i, _)| i)
                .unwrap_or(doc_text.len());
            format!(
                "{}|{}",
                crate::view::escape_insert(&doc_text[from..at]),
                crate::view::escape_insert(&doc_text[at..to])
            )
        }
        None => "<no caret>".to_string(),
    };
    let block = view.caret_block().and_then(|idx| {
        view.layout().and_then(|layout| {
            layout.blocks.get(idx).map(|b| {
                (
                    idx,
                    format!("{:?}", b.kind),
                    b.bounds.size.height.as_f32(),
                    b.line_height.as_f32(),
                )
            })
        })
    });
    let last = view.last_edit().cloned();
    NewlineStep {
        len_bytes: view.doc().len_bytes(),
        caret,
        anchor: view.anchor(),
        selection: view.selection_range(),
        revision: view.doc().revision(),
        trigger: view.last_edit_trigger().map(|t| t.as_str()),
        inserted: last.as_ref().map(|e| crate::view::escape_insert(&e.inserted)),
        block,
        trailing_newlines,
        window,
    }
}

fn describe_step(index: usize, step: &NewlineStep) -> String {
    let block = match &step.block {
        Some((idx, kind, height, line)) => {
            format!("block={idx} {kind} h={height:.1} line={line:.1}")
        }
        None => "block=<none>".to_string(),
    };
    format!(
        "#{:>2} buf={}B caret={:?} sel={:?} rev={} trigger={:?} insert=\"{}\" run=#{:<2} {block} text=\"{}\"",
        index,
        step.len_bytes,
        step.caret,
        step.selection,
        step.revision,
        step.trigger.unwrap_or("-"),
        step.inserted.clone().unwrap_or_else(|| "-".to_string()),
        step.trailing_newlines,
        step.window,
    )
}

/// Press Enter `presses` times through the real key path and assert each press.
///
/// `view` is the entity the window paints, `window` is that window: the keystroke
/// goes through the keymap and the `Newline` action exactly as a physical key
/// does, so this fails if the binding, the handler or the buffer path is wrong.
///
/// The view is always touched through `Entity::update(cx, ..)` and never held
/// across a dispatch: `dispatch_keystroke` re-enters the entity through the
/// action listener, so a live borrow would be a double borrow.
pub fn newlines_probe(
    view: &Entity<MarkdownView>,
    window: &mut AsyncWindowContext,
    target: Option<(usize, usize)>,
    presses: usize,
) -> ProbeCapture {
    let mut report = String::new();
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    // Place the caret. Default: the very end of the buffer, which is where a user
    // pressing Enter expects to keep going down.
    let placed = window
        .update(|_window, cx| {
            view.update(cx, |view, _cx| {
                let at = match target {
                    Some((block_idx, byte)) => view
                        .doc()
                        .blocks()
                        .get(block_idx)
                        .map(|b| b.src.start + byte.min(b.src.len()))
                        .unwrap_or_else(|| view.doc().len_bytes()),
                    None => view.doc().len_bytes(),
                };
                view.set_caret(at);
                newline_step(view)
            })
        })
        .map_err(|error| format!("{error:?}"));
    let first = match placed {
        Ok(first) => first,
        Err(error) => {
            checks.push((
                "N0 the window and the view are reachable".to_string(),
                false,
                format!("the window context could not be used: {error}"),
            ));
            return ProbeCapture { report, checks };
        }
    };
    report.push_str(&format!(
        "# newline probe: {presses} presses of the real `enter` keystroke\n\
         # start: {}\n\n",
        describe_step(0, &first),
    ));

    let mut steps: Vec<NewlineStep> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut dispatched_misses = 0usize;
    let mut previous = Some(first.clone());

    for press in 1..=presses {
        // 1. the key itself, through the dispatch tree
        let dispatched = window
            .update(|window, cx| window.dispatch_keystroke(enter_keystroke(), cx))
            .unwrap_or(false);
        if !dispatched {
            dispatched_misses += 1;
        }
        // 2. what it left behind (only the entity, never the window)
        let step = window
            .update(|_window, cx| view.update(cx, |view, _cx| newline_step(view)))
            .ok();
        let Some(after) = step else {
            failures.push(format!("press #{press}: the view is gone"));
            break;
        };
        let Some(before) = previous.clone() else {
            failures.push(format!("press #{press}: lost the previous state"));
            break;
        };
        report.push_str(&format!("{}\n", describe_step(press, &after)));

        if after.len_bytes != before.len_bytes + 1 {
            failures.push(format!(
                "press #{press}: buffer {}B -> {}B (expected +1)",
                before.len_bytes, after.len_bytes
            ));
        }
        if after.trailing_newlines != before.trailing_newlines + 1 {
            failures.push(format!(
                "press #{press}: newline run {} -> {} (expected +1)",
                before.trailing_newlines, after.trailing_newlines
            ));
        }
        match (before.caret, after.caret) {
            (Some(a), Some(b)) if b == a + 1 => {}
            other => failures.push(format!(
                "press #{press}: caret {other:?} did not advance by 1"
            )),
        }
        if after.trigger != Some("newline") {
            failures.push(format!(
                "press #{press}: the edit was tagged {:?}, not `newline`",
                after.trigger
            ));
        }
        if after.selection.is_some() {
            failures.push(format!(
                "press #{press}: a selection was live ({:?}) so Enter replaced it",
                after.selection
            ));
        }
        // Height: a *blank caret block* has to grow by one line per press. That
        // comparison only means something while the caret stays in the same block:
        // the second Enter after a paragraph creates the blank block in the first
        // place, and an empty document starts with no block at all.
        match (&before.block, &after.block) {
            (Some((idx_b, kind_b, h_b, line)), Some((idx_a, kind_a, h_a, _))) if idx_a == idx_b => {
                if kind_b == "Gap" && kind_a == "Gap" {
                    if (*h_a - *h_b - *line).abs() > 0.5 {
                        failures.push(format!(
                            "press #{press}: gap block {idx_a} height {h_b:.1} -> {h_a:.1}, expected +{line:.1}"
                        ));
                    }
                } else if *h_a < *h_b - 0.5 {
                    failures.push(format!(
                        "press #{press}: caret block {idx_b} height shrank {h_b:.1} -> {h_a:.1}"
                    ));
                }
            }
            (_, Some(_)) => {}
            (_, None) => failures.push(format!("press #{press}: no caret block")),
        }
        previous = Some(after.clone());
        steps.push(after);
    }

    // The whole sequence must leave `presses` newlines behind, in the buffer and
    // in the layout: that is "连续换行" stated as a number.
    let final_state = window
        .update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.sync_layout();
                let text = view.doc().text();
                let tail_newlines = text.chars().rev().take_while(|c| *c == '\n').count();
                let layout_height = view.layout().map(|l| l.size.height.as_f32()).unwrap_or(0.);
                (
                    text.len(),
                    tail_newlines,
                    layout_height,
                    view.doc().blocks().len(),
                )
            })
        })
        .ok();
    match &final_state {
        Some((len, tail_newlines, height, blocks)) => {
            report.push_str(&format!(
                "\n# end: {len}B, {tail_newlines} trailing newlines, page {height:.1}px high, {blocks} blocks\n"
            ));
            let expected = first.trailing_newlines + presses;
            if *tail_newlines != expected {
                failures.push(format!(
                    "after {presses} presses the buffer has {tail_newlines} trailing newlines, expected {expected}"
                ));
            }
        }
        None => failures.push("could not read the final buffer".to_string()),
    }

    checks.push((
        format!("N1 {presses} presses of `enter` add {presses} newlines and move the caret each time"),
        failures.is_empty(),
        if failures.is_empty() {
            format!(
                "{}B -> {}B, caret {:?} -> {:?}, {} keystrokes dispatched{}",
                first.len_bytes,
                steps.last().map_or(first.len_bytes, |s| s.len_bytes),
                first.caret,
                steps.last().and_then(|s| s.caret),
                presses,
                if dispatched_misses == 0 {
                    String::new()
                } else {
                    format!(" ({dispatched_misses} returned false)")
                }
            )
        } else {
            failures.join("; ")
        },
    ));

    ProbeCapture { report, checks }
}
