//! `--dump-media`: what the frame actually did with images and formulas.

use typors_core::doc::ObjectKind;
use typors_editor::ProbeCapture;
use typors_editor::media::ImageState;
use typors_editor::view::MarkdownView;

/// Inspect the objects of the last painted frame and return a transcript plus
/// pass/fail checks. Runs inside `paint`, after the layout was published.
pub fn media_probe(view: &MarkdownView) -> ProbeCapture {
    let mut report = String::from("-- media --\n");
    let mut failures: Vec<String> = Vec::new();
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    let Some(layout) = view.layout() else {
        failures.push("no layout".to_string());
        return finish(report, checks, failures);
    };
    let doc = view.doc();
    let content_width = layout.content_width.as_f32();
    let mut total = 0usize;
    let mut ready_images = 0usize;
    let mut failed = 0usize;
    let mut math_ok = 0usize;
    let mut diagrams = 0usize;

    for block in &layout.blocks {
        for object in &block.objects {
            total += 1;
            let source = doc
                .text()
                .get(object.source.clone())
                .unwrap_or_default()
                .to_string();
            let state = match &object.kind {
                ObjectKind::Image { destination, .. } => {
                    match view.media().image_state(destination) {
                        ImageState::Ready { width, height, .. } => {
                            ready_images += 1;
                            format!("ready {width:.0}x{height:.0}")
                        }
                        ImageState::Pending => "pending".to_string(),
                        ImageState::Failed(reason) => {
                            failed += 1;
                            format!("failed: {reason}")
                        }
                    }
                }
                ObjectKind::Math { tex, .. } => {
                    match view.media().math().svg(tex, object.font_size.as_f32()) {
                        Ok(_) => {
                            math_ok += 1;
                            "typeset".to_string()
                        }
                        Err(error) => {
                            failed += 1;
                            format!("math error: {error}")
                        }
                    }
                }
                ObjectKind::Diagram { source } => match view.media().diagram(source) {
                    Ok(diagram) => {
                        diagrams += 1;
                        format!("diagram {:.0}x{:.0}", diagram.width, diagram.height)
                    }
                    Err(error) => {
                        failed += 1;
                        format!("mermaid error: {error}")
                    }
                },
                ObjectKind::Chart { source } => match view.media().chart(source) {
                    Ok(chart) => {
                        diagrams += 1;
                        format!("chart {:.0}x{:.0}", chart.width, chart.height)
                    }
                    Err(error) => {
                        failed += 1;
                        format!("chart error: {error}")
                    }
                },
            };
            report.push_str(&format!(
                "  object block {} span {} {:?} src={:?} source={source:?} bounds=({:.1},{:.1} {:.1}x{:.1}) baseline={:.1} state={}\n",
                block.block_idx,
                object.span_idx,
                kind_label(&object.kind),
                object.source,
                object.bounds.origin.x.as_f32(),
                object.bounds.origin.y.as_f32(),
                object.bounds.size.width.as_f32(),
                object.bounds.size.height.as_f32(),
                object.baseline.as_f32(),
                state,
            ));

            // M1: the placeholder owns exactly one char, and the object's source
            // range round-trips through both projections.
            if let Some(span) = block
                .spans
                .iter()
                .find(|span| span.span_idx == object.span_idx)
            {
                if &block.render[span.render.clone()] != "\u{FFFC}" {
                    failures.push(format!(
                        "block {} span {}: object placeholder is {:?}",
                        block.block_idx,
                        object.span_idx,
                        &block.render[span.render.clone()]
                    ));
                }
                for src in object.source.start..=object.source.end {
                    if !doc.is_char_boundary(src) {
                        continue;
                    }
                    let pos = doc.src_to_render(src);
                    if doc.render_to_src(&pos) != Some(doc.snap_floor(src)) {
                        failures.push(format!(
                            "block {} span {}: source {src} does not round-trip",
                            block.block_idx, object.span_idx
                        ));
                        break;
                    }
                }
            } else if !matches!(
                &object.kind,
                ObjectKind::Image { .. } | ObjectKind::Math { .. }
            ) {
                failures.push(format!(
                    "block {} object span {} has no span table entry",
                    block.block_idx, object.span_idx
                ));
            }

            // M2: a ready image keeps its aspect ratio and never exceeds the
            // column. A placeholder is allowed to be the fixed 320x180 box.
            if let ObjectKind::Image { destination, .. } = &object.kind
                && let ImageState::Ready { width, height, .. } =
                    view.media().image_state(destination)
            {
                let expected_ratio = width / height;
                let actual_ratio =
                    object.bounds.size.width.as_f32() / object.bounds.size.height.as_f32();
                if (expected_ratio - actual_ratio).abs() > 0.01 {
                    failures.push(format!(
                        "block {} image aspect {actual_ratio:.3} != {expected_ratio:.3}",
                        block.block_idx
                    ));
                }
                if object.bounds.size.width.as_f32() > content_width + 0.5 {
                    failures.push(format!(
                        "block {} image wider than the column: {} > {}",
                        block.block_idx,
                        object.bounds.size.width.as_f32(),
                        content_width
                    ));
                }
            }

            // M3: math geometry agrees with the engine's measurement.
            if let ObjectKind::Math { tex, .. } = &object.kind
                && let Ok(metrics) = view.media().math().measure(tex, object.font_size.as_f32())
            {
                let expected_height = metrics.total_height_px();
                if (object.bounds.size.height.as_f32() - expected_height).abs() > 1.0 {
                    failures.push(format!(
                        "block {} math height {} != {expected_height}",
                        block.block_idx,
                        object.bounds.size.height.as_f32()
                    ));
                }
            }
        }
    }

    // M4: failures keep a non-zero box (the view paints a placeholder or the
    // source fallback, never nothing).
    if block_objects_without_bounds(layout) {
        failures.push("an object has a zero-sized box".to_string());
    }

    checks.push((
        "M1 objects have one-char placeholders and lossless source mapping".to_string(),
        failures
            .iter()
            .all(|f| !f.contains("placeholder") && !f.contains("round-trip")),
        format!("{total} objects"),
    ));
    checks.push((
        "M2 ready images preserve aspect ratio and stay within the column".to_string(),
        failures
            .iter()
            .all(|f| !f.contains("aspect") && !f.contains("wider")),
        format!("{ready_images} ready images"),
    ));
    checks.push((
        "M3 math geometry matches the engine (<=1px)".to_string(),
        failures.iter().all(|f| !f.contains("math height")),
        format!("{math_ok} typeset formulas"),
    ));
    checks.push((
        "M4 failed/loading objects keep a non-zero placeholder box".to_string(),
        !block_objects_without_bounds(layout),
        format!("{failed} failed objects"),
    ));
    let minimum = ready_images + math_ok;
    let maximum = minimum + diagrams;
    checks.push((
        "M5 every visible ready/typeset object produced a paint command".to_string(),
        view.painted_objects() >= minimum && view.painted_objects() <= maximum,
        format!(
            "{} commands, visible minimum {minimum}, upper bound {maximum} (off-screen diagrams are culled)",
            view.painted_objects(),
        ),
    ));

    finish(report, checks, failures)
}

fn kind_label(kind: &ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Image { .. } => "image",
        ObjectKind::Math { display: true, .. } => "display-math",
        ObjectKind::Math { display: false, .. } => "inline-math",
        ObjectKind::Diagram { .. } => "diagram",
        ObjectKind::Chart { .. } => "chart",
    }
}

fn block_objects_without_bounds(layout: &typors_render::DocLayout) -> bool {
    layout.blocks.iter().any(|block| {
        block.objects.iter().any(|object| {
            object.bounds.size.width.as_f32() <= 0.0 || object.bounds.size.height.as_f32() <= 0.0
        })
    })
}

fn finish(
    report: String,
    checks: Vec<(String, bool, String)>,
    failures: Vec<String>,
) -> ProbeCapture {
    let mut text = report;
    for failure in &failures {
        text.push_str(&format!("  [detail] {failure}\n"));
    }
    ProbeCapture {
        report: text,
        checks,
    }
}
