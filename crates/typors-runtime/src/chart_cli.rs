//! Chart probe: the bar matrix and the reference document's examples.

use typors_media::chart;

/// Run for --dump-chart; the probe needs no window.
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--dump-chart") {
        return Some(dump());
    }
    None
}

/// Bars as (x, y, width, height), ignoring the background and legend chips.
fn bars(svg: &str) -> Vec<(f32, f32, f32, f32)> {
    let mut out = Vec::new();
    for part in svg.split("<rect ").skip(1) {
        let attr = |name: &str| -> Option<f32> {
            let start = part.find(&format!("{name}=\""))? + name.len() + 2;
            let rest = &part[start..];
            let end = rest.find('"')?;
            rest[..end].parse::<f32>().ok()
        };
        let (Some(x), Some(y), Some(width), Some(height)) =
            (attr("x"), attr("y"), attr("width"), attr("height"))
        else {
            continue;
        };
        if width == 10.0 && height == 10.0 {
            continue;
        }
        out.push((x, y, width, height));
    }
    out
}

fn dump() -> i32 {
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let render = |source: &str| chart::render(source).map(|chart| chart.svg);

    let grouped = render("bar\n\nq1: [12, 18, 9]\nq2: [8, 14, 19]\n").unwrap_or_default();
    let grouped_bars = bars(&grouped);
    checks.push((
        "C1 grouped bars: two series over three categories".to_string(),
        grouped_bars.len() == 6
            && (grouped_bars[0].0 - grouped_bars[1].0).abs() > 1.0
            && (grouped_bars[0].2 - grouped_bars[1].2).abs() < 0.01,
        format!("{} bars, first pair {:?}", grouped_bars.len(), grouped_bars.first()),
    ));

    let labelled = render("bar\n\nlabels: [一月, 二月, 三月]\ns: [1, 2, 3]\n").unwrap_or_default();
    checks.push((
        "C2 category labels reach the axis".to_string(),
        ["一月", "二月", "三月"].iter().all(|label| labelled.contains(label)),
        "three CJK labels".to_string(),
    ));

    let negative = render("bar\n\ns: [-5, 10]\n").unwrap_or_default();
    let zero_line = negative
        .split("<line ")
        .find(|part| part.contains("stroke-width=\"1.5\""))
        .and_then(|part| {
            let start = part.find("y1=\"")? + 4;
            let rest = &part[start..];
            rest[..rest.find('"')?].parse::<f32>().ok()
        });
    let negative_bar = bars(&negative)
        .into_iter()
        .find(|bar| bar.1 >= zero_line.unwrap_or(f32::MAX) - 0.5);
    checks.push((
        "C3 negative values draw below a zero baseline".to_string(),
        zero_line.is_some() && negative_bar.is_some(),
        format!("zero {zero_line:?}, bar {negative_bar:?}"),
    ));

    let valued = render("bar\n\nvalues: true\ns: [12, 18]\n").unwrap_or_default();
    let plain = render("bar\n\ns: [12, 18]\n").unwrap_or_default();
    checks.push((
        "C4 value labels are optional".to_string(),
        valued.contains(">12<") && !plain.contains(">12<"),
        "global values".to_string(),
    ));

    let horizontal =
        render("bar\n\nhorizontal: true\nlabels: [A, B]\ns: [3, 8]\n").unwrap_or_default();
    let horizontal_bars = bars(&horizontal);
    checks.push((
        "C5 horizontal bars are wide".to_string(),
        horizontal_bars.len() == 2 && horizontal_bars.iter().all(|bar| bar.2 > bar.3),
        format!("{:?}", horizontal_bars),
    ));

    let stacked = render("bar\n\nstacked: true\ns1: [3]\ns2: [4]\n").unwrap_or_default();
    let stacked_bars = bars(&stacked);
    let stacked_ok = if stacked_bars.len() == 2 {
        let (a, b) = (stacked_bars[0], stacked_bars[1]);
        let (upper, lower) = if a.1 < b.1 { (a, b) } else { (b, a) };
        (a.0 - b.0).abs() < 0.01 && (upper.1 + upper.3 - lower.1).abs() < 0.5
    } else {
        false
    };
    checks.push((
        "C6 stacked segments sit end to end".to_string(),
        stacked_ok,
        format!("{:?}", stacked_bars),
    ));

    let unknown = render("bar\n\ns: [1]\ns.colour: #ff0000\n");
    checks.push((
        "C7 unknown series options are errors".to_string(),
        unknown.is_err(),
        format!("{unknown:?}"),
    ));

    // The reference document's examples are the public contract. A source tree
    // has docs/chart.md; an installed copy may not, which is a skip (the crate's
    // own test embeds the document and enforces the same contract at build time).
    let doc_path = chart_doc_path();
    let examples = doc_path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|doc| chart_examples(&doc));
    let mut failed = Vec::new();
    if let Some(examples) = &examples {
        for (index, source) in examples.iter().enumerate() {
            if let Err(error) = chart::render(source) {
                failed.push(format!("example {index}: {error}"));
            }
        }
    }
    let (c8_pass, c8_detail) = match (&doc_path, &examples) {
        (Some(path), Some(examples)) => (
            !examples.is_empty() && failed.is_empty(),
            format!(
                "{}, {} examples, {} failed: {}",
                path.display(),
                examples.len(),
                failed.len(),
                failed.join("; ")
            ),
        ),
        (Some(path), None) => (false, format!("cannot read {}", path.display())),
        _ => (true, "skipped: no docs/chart.md beside the executable".to_string()),
    };
    checks.push((
        "C8 every example in docs/chart.md renders".to_string(),
        c8_pass,
        c8_detail,
    ));

    let ok = checks.iter().all(|(_, pass, _)| *pass);
    println!("# chart dump\n");
    for (name, pass, detail) in &checks {
        println!("  [{}] {name} — {detail}", if *pass { "ok" } else { "FAIL" });
    }
    println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}

/// Locate docs/chart.md: an override, the working directory, or the source tree
/// two levels above target/<profile>/.
fn chart_doc_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("TYPORS_CHART_DOC") {
        return Some(std::path::PathBuf::from(path));
    }
    let mut candidates = vec![std::path::PathBuf::from("docs/chart.md")];
    if let Some(exe) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|p| p.to_path_buf())) {
        candidates.push(exe.join("../../docs/chart.md"));
        candidates.push(exe.join("docs/chart.md"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn chart_examples(doc: &str) -> Vec<String> {
    let mut examples = Vec::new();
    let mut current: Option<String> = None;
    for line in doc.lines() {
        let trimmed = line.trim();
        if current.is_none() && trimmed == "```chart" {
            current = Some(String::new());
            continue;
        }
        if let Some(source) = &mut current {
            if trimmed == "```" {
                examples.push(source.clone());
                current = None;
            } else {
                source.push_str(line);
                source.push('\n');
            }
        }
    }
    examples
}
