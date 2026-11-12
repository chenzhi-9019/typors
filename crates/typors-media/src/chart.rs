//! Native charts for fenced ```chart blocks.
//!
//! The syntax is deliberately close to Matplotlib's vocabulary: a chart type
//! line, then `series: value` assignments and `series.option: value` options.
//! Supported types: pie, line, bar, scatter. No JavaScript, no browser.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct Chart {
    pub svg: String,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartKind {
    Pie,
    Line,
    Bar,
    Scatter,
}

#[derive(Clone, Debug, Default)]
struct Series {
    values: Vec<f64>,
    labels: Vec<String>,
    color: Option<String>,
    label: Option<String>,
    marker: Option<String>,
    line: bool,
    percent: bool,
    /// Print each bar's value (per-series form of the global option).
    value_labels: bool,
}

#[derive(Clone, Debug)]
struct Spec {
    kind: ChartKind,
    series: BTreeMap<String, Series>,
    title: Option<String>,
    width: f32,
    height: f32,
    /// Category axis labels shared by every series.
    labels: Option<Vec<String>>,
    stacked: bool,
    horizontal: bool,
    values: bool,
}

const DEFAULT_COLORS: &[&str] = &[
    "#4E79A7", "#F28E2B", "#E15759", "#76B7B2", "#59A14F", "#EDC948", "#B07AA1", "#FF9DA7",
    "#9C755F", "#BAB0AC",
];
const UNDEFINED: &str = "#B0B0B0";

/// Every option the parser accepts, with its scope. The reference documentation
/// is checked against this list, so an option cannot be added or removed without
/// updating docs/chart.md in the same change.
pub const OPTIONS: &[(&str, &str)] = &[
    ("title", "global"),
    ("width", "global"),
    ("height", "global"),
    ("labels", "global"),
    ("stacked", "global"),
    ("horizontal", "global"),
    ("values", "global"),
    ("color", "series"),
    ("label", "series"),
    ("marker", "series"),
    ("line", "series"),
    ("values", "series"),
    ("x", "series"),
    ("labels", "series"),
];

pub fn render(source: &str) -> Result<Chart, String> {
    let spec = parse(source)?;
    let svg = match spec.kind {
        ChartKind::Pie => pie(&spec),
        ChartKind::Bar => bar(&spec),
        ChartKind::Line | ChartKind::Scatter => cartesian(&spec),
    };
    Ok(Chart {
        svg,
        width: spec.width,
        height: spec.height,
    })
}

fn parse(source: &str) -> Result<Spec, String> {
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let kind = match lines.next().map(|line| line.to_ascii_lowercase()) {
        Some(kind) if kind == "pie" => ChartKind::Pie,
        Some(kind) if kind == "line" => ChartKind::Line,
        Some(kind) if kind == "bar" => ChartKind::Bar,
        Some(kind) if kind == "scatter" => ChartKind::Scatter,
        _ => return Err("chart: first line must be pie/line/bar/scatter".to_string()),
    };
    let mut spec = Spec {
        kind,
        series: BTreeMap::new(),
        title: None,
        width: if kind == ChartKind::Pie { 440.0 } else { 560.0 },
        height: if kind == ChartKind::Pie { 300.0 } else { 340.0 },
        labels: None,
        stacked: false,
        horizontal: false,
        values: false,
    };
    for (index, line) in lines.enumerate() {
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("chart line {}: expected key: value", index + 2));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            return Err(format!("chart line {}: empty key", index + 2));
        }
        if key.eq_ignore_ascii_case("title") {
            spec.title = Some(string_value(value));
            continue;
        }
        if key.eq_ignore_ascii_case("width") {
            spec.width = value
                .parse::<f32>()
                .unwrap_or(spec.width)
                .clamp(240.0, 1600.0);
            continue;
        }
        if key.eq_ignore_ascii_case("height") {
            spec.height = value
                .parse::<f32>()
                .unwrap_or(spec.height)
                .clamp(160.0, 1200.0);
            continue;
        }
        // Reserved global options win over a series of the same name.
        if key.eq_ignore_ascii_case("labels") {
            spec.labels = Some(parse_labels(value));
            continue;
        }
        if key.eq_ignore_ascii_case("stacked") {
            spec.stacked = truthy(value);
            continue;
        }
        if key.eq_ignore_ascii_case("horizontal") {
            spec.horizontal = truthy(value);
            continue;
        }
        if key.eq_ignore_ascii_case("values") {
            spec.values = truthy(value);
            continue;
        }
        let (name, option) = key.split_once('.').unwrap_or((key, ""));
        let series = spec.series.entry(name.to_string()).or_default();
        if option.is_empty() {
            if let Some(array) = parse_array(value) {
                series.values = array;
            } else if let Some(number) = parse_number(value) {
                series.values = vec![number];
                series.percent = value.trim_end().ends_with('%');
            } else {
                series.labels.push(string_value(value));
            }
            continue;
        }
        match option.to_ascii_lowercase().as_str() {
            "color" => series.color = Some(string_value(value)),
            "label" => series.label = Some(string_value(value)),
            "marker" => series.marker = Some(string_value(value)),
            "line" => series.line = truthy(value),
            "values" => series.value_labels = truthy(value),
            "x" | "labels" => series.labels = parse_labels(value),
            other => {
                return Err(format!(
                    "chart line {}: unknown option '{}' on series '{}'",
                    index + 2,
                    other,
                    name
                ));
            }
        }
    }
    if spec.series.is_empty() {
        return Err("chart: no series defined".to_string());
    }
    Ok(spec)
}

fn truthy(value: &str) -> bool {
    value.eq_ignore_ascii_case("true") || value == "1" || value.eq_ignore_ascii_case("yes")
}

/// A bracketed label list, or a single label.
fn parse_labels(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.starts_with('[') && value.ends_with(']') {
        value
            .trim_matches(['[', ']'])
            .split(',')
            .map(|part| string_value(part.trim()))
            .collect()
    } else {
        vec![string_value(value)]
    }
}

fn string_value(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
        .to_string()
}

fn parse_number(value: &str) -> Option<f64> {
    value
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse::<f64>()
        .ok()
}

fn parse_array(value: &str) -> Option<Vec<f64>> {
    let inner = value.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner
        .split(',')
        .map(|part| parse_number(part))
        .collect::<Option<Vec<_>>>()
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn pie(spec: &Spec) -> String {
    let width = spec.width;
    let height = spec.height;
    let cx = width * 0.32;
    let cy = height * 0.52;
    let r = height.min(width) * 0.34;
    let mut entries: Vec<(String, f64, String, String)> = Vec::new();
    let mut total = 0.0f64;
    let mut percent_mode = false;
    for (index, (name, series)) in spec.series.iter().enumerate() {
        let value = series.values.first().copied().unwrap_or(0.0).max(0.0);
        if value > 0.0 {
            percent_mode |= series.percent;
            total += value;
            entries.push((
                series.label.clone().unwrap_or_else(|| name.clone()),
                value,
                series
                    .color
                    .clone()
                    .unwrap_or_else(|| DEFAULT_COLORS[index % DEFAULT_COLORS.len()].to_string()),
                name.clone(),
            ));
        }
    }
    if total <= 0.0 {
        return empty_svg(width, height, "no positive values");
    }
    let denominator = if percent_mode && total < 100.0 {
        100.0
    } else {
        total
    };
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="#ffffff"/>"##
    );
    if let Some(title) = &spec.title {
        let _ = write!(
            svg,
            r##"<text x="{:.0}" y="22" font-family="Segoe UI, sans-serif" font-size="14" font-weight="600" fill="#333">{}</text>"##,
            width / 2.0,
            esc(title)
        );
    }
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (label, value, color, _) in &entries {
        let sweep = (value / denominator) * std::f64::consts::TAU;
        let large = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let x1 = cx as f64 + r as f64 * angle.cos();
        let y1 = cy as f64 + r as f64 * angle.sin();
        let end = angle + sweep;
        let x2 = cx as f64 + r as f64 * end.cos();
        let y2 = cy as f64 + r as f64 * end.sin();
        let _ = write!(
            svg,
            r##"<path d="M{cx:.1},{cy:.1} L{x1:.2},{y1:.2} A{r:.2},{r:.2} 0 {large},1 {x2:.2},{y2:.2} Z" fill="{color}" stroke="#ffffff" stroke-width="1"/>"##
        );
        angle = end;
        if !label.is_empty() {
            let mid = (angle - sweep / 2.0) as f32;
            let lx = cx + r * 0.62 * mid.cos();
            let ly = cy + r * 0.62 * mid.sin();
            let _ = write!(
                svg,
                r##"<text x="{lx:.1}" y="{ly:.1}" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="11" fill="#ffffff">{}</text>"##,
                esc(label)
            );
        }
    }
    if denominator > total {
        let sweep = ((denominator - total) / denominator) * std::f64::consts::TAU;
        let large = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let x1 = cx as f64 + r as f64 * angle.cos();
        let y1 = cy as f64 + r as f64 * angle.sin();
        let end = angle + sweep;
        let x2 = cx as f64 + r as f64 * end.cos();
        let y2 = cy as f64 + r as f64 * end.sin();
        let _ = write!(
            svg,
            r##"<path d="M{cx:.1},{cy:.1} L{x1:.2},{y1:.2} A{r:.2},{r:.2} 0 {large},1 {x2:.2},{y2:.2} Z" fill="{UNDEFINED}" stroke="#ffffff" stroke-width="1"/>"##
        );
    }
    let mut y = 58.0;
    for (label, _, color, _) in &entries {
        if label.is_empty() {
            continue;
        }
        let _ = write!(
            svg,
            r##"<rect x="{:.0}" y="{y:.0}" width="10" height="10" fill="{color}"/><text x="{:.0}" y="{:.0}" font-family="Segoe UI, sans-serif" font-size="12" fill="#333">{}</text>"##,
            width * 0.58,
            width * 0.58 + 16.0,
            y + 9.0,
            esc(label)
        );
        y += 20.0;
    }
    svg.push_str("</svg>");
    svg
}

/// Approximate text width for label layout. typors-media has no text system, so
/// labels are measured with the same character classes as ApproxMetrics.
fn approx_width(text: &str, font_size: f32) -> f32 {
    text.chars()
        .map(|ch| {
            let em = match ch as u32 {
                0x1100..=0x115F
                | 0x2E80..=0xA4CF
                | 0xAC00..=0xD7A3
                | 0xF900..=0xFAFF
                | 0xFE30..=0xFE6F
                | 0xFF00..=0xFF60
                | 0xFFE0..=0xFFE6
                | 0x1F300..=0x1FAFF => 1.0,
                _ => 0.55,
            };
            em * font_size
        })
        .sum()
}

/// Shorten a label to max_width with an ellipsis, never splitting a character.
fn truncate_to_width(text: &str, max_width: f32, font_size: f32) -> String {
    if approx_width(text, font_size) <= max_width {
        return text.to_string();
    }
    let ellipsis_width = approx_width("…", font_size);
    let mut out = String::new();
    let mut width = 0.0;
    for ch in text.chars() {
        let next = approx_width(ch.encode_utf8(&mut [0; 4]), font_size);
        if width + next + ellipsis_width > max_width {
            break;
        }
        out.push(ch);
        width += next;
    }
    out.push('…');
    out
}

fn format_value(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Horizontal category labels: global labels, else the first series that has
/// them, else 1-based indices.
fn category_labels(spec: &Spec, categories: usize) -> Vec<String> {
    if let Some(labels) = &spec.labels {
        return (0..categories)
            .map(|index| labels.get(index).cloned().unwrap_or_else(|| (index + 1).to_string()))
            .collect();
    }
    for series in spec.series.values() {
        if !series.labels.is_empty() {
            return (0..categories)
                .map(|index| {
                    series
                        .labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| (index + 1).to_string())
                })
                .collect();
        }
    }
    (0..categories).map(|index| (index + 1).to_string()).collect()
}

/// Value range that always contains zero. Stacked charts range over the stacked
/// totals (positive and negative separately).
fn bar_value_range(spec: &Spec, categories: usize) -> (f64, f64) {
    let mut min = 0.0f64;
    let mut max = 0.0f64;
    if spec.stacked {
        for category in 0..categories {
            let mut positive = 0.0;
            let mut negative = 0.0;
            for series in spec.series.values() {
                let value = series.values.get(category).copied().unwrap_or(0.0);
                if value >= 0.0 {
                    positive += value;
                } else {
                    negative += value;
                }
            }
            min = min.min(negative);
            max = max.max(positive);
        }
    } else {
        for series in spec.series.values() {
            for value in series.values.iter().copied().filter(|v| v.is_finite()) {
                min = min.min(value);
                max = max.max(value);
            }
        }
    }
    if min == 0.0 && max == 0.0 {
        max = 1.0;
    }
    (min, max)
}

/// Native bar chart: grouped by default, with category tick labels, value labels,
/// a zero baseline for negatives, and optional horizontal / stacked layout.
fn bar(spec: &Spec) -> String {
    let width = spec.width;
    let height = spec.height;
    let categories = spec
        .series
        .values()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(0);
    if categories == 0 {
        return empty_svg(width, height, "no data");
    }
    let (min, max) = bar_value_range(spec, categories);
    if !min.is_finite() || !max.is_finite() {
        return empty_svg(width, height, "no data");
    }
    let span = (max - min).max(1e-9);
    let labels = category_labels(spec, categories);
    let horizontal = spec.horizontal;
    let series_count = spec.series.len().max(1);

    let title_space = if spec.title.is_some() { 32.0 } else { 16.0 };
    let (left, right, top, bottom) = if horizontal {
        (96.0, width - 20.0, title_space + 8.0, height - 46.0)
    } else {
        (52.0, width - 20.0, title_space + 8.0, height - 54.0)
    };
    let plot_w = (right - left).max(1.0);
    let plot_h = (bottom - top).max(1.0);

    let tick = |index: usize| -> f64 { min + span * index as f64 / 4.0 };
    let value_px = |value: f64| -> f32 {
        if horizontal {
            left + (((value - min) / span) as f32) * plot_w
        } else {
            bottom - (((value - min) / span) as f32) * plot_h
        }
    };
    let category_px = |index: usize| -> f32 {
        if horizontal {
            top + (index as f32 + 0.5) * plot_h / categories as f32
        } else {
            left + (index as f32 + 0.5) * plot_w / categories as f32
        }
    };

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="#ffffff"/>"##
    );
    if let Some(title) = &spec.title {
        let _ = write!(
            svg,
            r##"<text x="{:.0}" y="22" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="14" font-weight="600" fill="#333">{}</text>"##,
            width / 2.0,
            esc(title)
        );
    }

    // Gridlines and value ticks.
    for index in 0..=4 {
        let value = tick(index);
        let position = value_px(value);
        let (x1, y1, x2, y2) = if horizontal {
            (position, top, position, bottom)
        } else {
            (left, position, right, position)
        };
        let _ = write!(
            svg,
            r##"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="#eeeeee"/>"##
        );
        if horizontal {
            let _ = write!(
                svg,
                r##"<text x="{position:.1}" y="{:.1}" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="10" fill="#777">{}</text>"##,
                bottom + 16.0,
                format_value(value)
            );
        } else {
            let _ = write!(
                svg,
                r##"<text x="{:.1}" y="{:.1}" text-anchor="end" font-family="Segoe UI, sans-serif" font-size="10" fill="#777">{}</text>"##,
                left - 6.0,
                position + 3.0,
                format_value(value)
            );
        }
    }

    // Axes and the zero baseline.
    let zero = value_px(0.0);
    if horizontal {
        let _ = write!(
            svg,
            r##"<line x1="{left:.1}" y1="{top:.1}" x2="{left:.1}" y2="{bottom:.1}" stroke="#999"/><line x1="{left:.1}" y1="{bottom:.1}" x2="{right:.1}" y2="{bottom:.1}" stroke="#999"/>"##
        );
        if min < 0.0 && max > 0.0 {
            let _ = write!(
                svg,
                r##"<line x1="{zero:.1}" y1="{top:.1}" x2="{zero:.1}" y2="{bottom:.1}" stroke="#999" stroke-width="1.5"/>"##
            );
        }
    } else {
        let _ = write!(
            svg,
            r##"<line x1="{left:.1}" y1="{top:.1}" x2="{left:.1}" y2="{bottom:.1}" stroke="#999"/><line x1="{left:.1}" y1="{bottom:.1}" x2="{right:.1}" y2="{bottom:.1}" stroke="#999"/>"##
        );
        if min < 0.0 && max > 0.0 {
            let _ = write!(
                svg,
                r##"<line x1="{left:.1}" y1="{zero:.1}" x2="{right:.1}" y2="{zero:.1}" stroke="#999" stroke-width="1.5"/>"##
            );
        }
    }

    // Group geometry: series are drawn side by side within a category slot.
    let slot = if horizontal { plot_h } else { plot_w } / categories as f32;
    let group = slot * 0.72;
    // Stacked series share one column; grouped series split the slot.
    let columns = if spec.stacked { 1 } else { series_count };
    let bar_thickness = (group / columns as f32 - 2.0).clamp(2.0, 36.0);
    let offset = |series_index: usize| -> f32 {
        if spec.stacked {
            0.0
        } else {
            (series_index as f32 - (series_count as f32 - 1.0) / 2.0) * (bar_thickness + 2.0)
        }
    };

    let mut positive_stack = vec![0.0f64; categories];
    let mut negative_stack = vec![0.0f64; categories];

    for (series_index, series) in spec.series.values().enumerate() {
        let color = series
            .color
            .clone()
            .unwrap_or_else(|| DEFAULT_COLORS[series_index % DEFAULT_COLORS.len()].to_string());
        let show_values = series.value_labels || spec.values;
        for (category, value) in series.values.iter().copied().enumerate() {
            if !value.is_finite() {
                continue;
            }
            let (from, to) = if spec.stacked {
                let stack = if value >= 0.0 {
                    &mut positive_stack[category]
                } else {
                    &mut negative_stack[category]
                };
                let from = *stack;
                *stack += value;
                (from, from + value)
            } else {
                (0.0, value)
            };
            let start = value_px(from);
            let end = value_px(to);
            let center = category_px(category) + offset(series_index);
            let (x, y, w, h) = if horizontal {
                (
                    start.min(end),
                    center - bar_thickness / 2.0,
                    (start - end).abs(),
                    bar_thickness,
                )
            } else {
                (
                    center - bar_thickness / 2.0,
                    start.min(end),
                    bar_thickness,
                    (start - end).abs(),
                )
            };
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let _ = write!(
                svg,
                r##"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{color}"/>"##
            );
            if show_values {
                let (text_x, text_y, anchor) = if horizontal {
                    if value >= 0.0 {
                        (x + w + 4.0, y + h / 2.0 + 4.0, "start")
                    } else {
                        (x - 4.0, y + h / 2.0 + 4.0, "end")
                    }
                } else if value >= 0.0 {
                    (x + w / 2.0, y - 4.0, "middle")
                } else {
                    (x + w / 2.0, y + h + 12.0, "middle")
                };
                let fits = if horizontal { w >= 12.0 } else { h >= 12.0 };
                if fits {
                    let _ = write!(
                        svg,
                        r##"<text x="{text_x:.1}" y="{text_y:.1}" text-anchor="{anchor}" font-family="Segoe UI, sans-serif" font-size="10" fill="#444">{}</text>"##,
                        format_value(value)
                    );
                }
            }
        }
    }

    // Category tick labels.
    let category_font = 11.0;
    for (index, label) in labels.iter().enumerate() {
        let center = category_px(index);
        let text = if horizontal {
            truncate_to_width(label, left - 16.0, category_font)
        } else {
            truncate_to_width(label, slot * 0.92, category_font)
        };
        let (x, y, anchor) = if horizontal {
            (left - 8.0, center + 4.0, "end")
        } else {
            (center, bottom + 16.0, "middle")
        };
        let _ = write!(
            svg,
            r##"<text x="{x:.1}" y="{y:.1}" text-anchor="{anchor}" font-family="Segoe UI, sans-serif" font-size="{category_font}" fill="#555">{}</text>"##,
            esc(&text)
        );
    }

    // Legend, top right, one row per series.
    let mut y = top + 4.0;
    for (series_index, series) in spec.series.values().enumerate() {
        let label = series.label.as_deref().unwrap_or("series");
        let color = series
            .color
            .clone()
            .unwrap_or_else(|| DEFAULT_COLORS[series_index % DEFAULT_COLORS.len()].to_string());
        let _ = write!(
            svg,
            r##"<rect x="{:.0}" y="{y:.0}" width="10" height="10" fill="{color}"/><text x="{:.0}" y="{:.0}" font-family="Segoe UI, sans-serif" font-size="11" fill="#333">{}</text>"##,
            right - 120.0,
            right - 104.0,
            y + 9.0,
            esc(label)
        );
        y += 16.0;
    }
    svg.push_str("</svg>");
    svg
}

fn cartesian(spec: &Spec) -> String {
    let width = spec.width;
    let height = spec.height;
    let left = 52.0;
    let right = width - 20.0;
    let top = 34.0;
    let bottom = height - 42.0;
    let mut values: Vec<f64> = Vec::new();
    for series in spec.series.values() {
        values.extend(series.values.iter().copied().filter(|v| v.is_finite()));
    }
    if values.is_empty() {
        return empty_svg(width, height, "no data");
    }
    let min = values
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min)
        .min(0.0);
    let max = values
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
        .max(1.0);
    let span = (max - min).max(1e-9);
    let max_len = spec
        .series
        .values()
        .map(|s| s.values.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let px = |index: usize| -> f32 {
        if max_len <= 1 {
            (left + right) / 2.0
        } else {
            left + (right - left) * index as f32 / (max_len - 1) as f32
        }
    };
    let py = |value: f64| -> f32 { bottom - (((value - min) / span) as f32) * (bottom - top) };
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="#ffffff"/>"##
    );
    if let Some(title) = &spec.title {
        let _ = write!(
            svg,
            r##"<text x="{:.0}" y="22" font-family="Segoe UI, sans-serif" font-size="14" font-weight="600" fill="#333">{}</text>"##,
            width / 2.0,
            esc(title)
        );
    }
    for tick in 0..=4 {
        let value = min + span * tick as f64 / 4.0;
        let y = py(value);
        let _ = write!(
            svg,
            r##"<line x1="{left:.1}" y1="{y:.1}" x2="{right:.1}" y2="{y:.1}" stroke="#eeeeee"/><text x="{:.1}" y="{:.1}" text-anchor="end" font-family="Segoe UI, sans-serif" font-size="10" fill="#777">{value:.2}</text>"##,
            left - 6.0,
            y + 3.0
        );
    }
    let _ = write!(
        svg,
        r##"<line x1="{left:.1}" y1="{top:.1}" x2="{left:.1}" y2="{bottom:.1}" stroke="#999"/><line x1="{left:.1}" y1="{bottom:.1}" x2="{right:.1}" y2="{bottom:.1}" stroke="#999"/>"##
    );
    for (series_index, series) in spec.series.values().enumerate() {
        let color = series
            .color
            .clone()
            .unwrap_or_else(|| DEFAULT_COLORS[series_index % DEFAULT_COLORS.len()].to_string());
        if spec.kind == ChartKind::Bar {
            let slot = (right - left) / max_len as f32;
            let bar_width = (slot * 0.6).max(4.0);
            for (index, value) in series.values.iter().enumerate() {
                let x = px(index) - bar_width / 2.0;
                let y = py(*value);
                let _ = write!(
                    svg,
                    r##"<rect x="{x:.1}" y="{y:.1}" width="{bar_width:.1}" height="{:.1}" fill="{color}"/>"##,
                    (bottom - y).max(0.0)
                );
            }
            continue;
        }
        let points: Vec<(f32, f32)> = series
            .values
            .iter()
            .enumerate()
            .map(|(index, value)| (px(index), py(*value)))
            .collect();
        if spec.kind == ChartKind::Line || series.line {
            let mut path = String::new();
            for (index, (x, y)) in points.iter().enumerate() {
                if index == 0 {
                    let _ = write!(path, "M{x:.1},{y:.1}");
                } else {
                    let _ = write!(path, " L{x:.1},{y:.1}");
                }
            }
            let _ = write!(
                svg,
                r##"<path d="{path}" fill="none" stroke="{color}" stroke-width="2"/>"##
            );
        }
        for (x, y) in &points {
            let _ = write!(
                svg,
                r##"<circle cx="{x:.1}" cy="{y:.1}" r="3" fill="{color}"/>"##
            );
            if let Some(marker) = &series.marker {
                let _ = write!(
                    svg,
                    r##"<text x="{x:.1}" y="{:.1}" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="12" fill="{color}">{}</text>"##,
                    y - 6.0,
                    esc(marker)
                );
            }
        }
    }
    let mut y = 46.0;
    for (index, series) in spec.series.values().enumerate() {
        let label = series.label.as_deref().unwrap_or("series");
        let color = series
            .color
            .clone()
            .unwrap_or_else(|| DEFAULT_COLORS[index % DEFAULT_COLORS.len()].to_string());
        let _ = write!(
            svg,
            r##"<rect x="{:.0}" y="{y:.0}" width="10" height="10" fill="{color}"/><text x="{:.0}" y="{:.0}" font-family="Segoe UI, sans-serif" font-size="11" fill="#333">{}</text>"##,
            right - 120.0,
            right - 104.0,
            y + 9.0,
            esc(label)
        );
        y += 16.0;
    }
    svg.push_str("</svg>");
    svg
}

fn empty_svg(width: f32, height: f32, message: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="#fafafa"/><text x="{:.0}" y="{:.0}" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="13" fill="#999">{}</text></svg>"##,
        width / 2.0,
        height / 2.0,
        esc(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pie_adds_gray_undefined_remainder() {
        let svg = render("pie\n\na: 10%\nb: 30%\nc: 50%\n\na.color: #FF0000\na.label: Test\n")
            .unwrap()
            .svg;
        assert!(svg.contains("#FF0000"));
        assert!(svg.contains(UNDEFINED));
        assert!(svg.contains("Test"));
    }

    #[test]
    fn line_uses_series_options() {
        let svg = render(
            "line\n\na: [1, 2, 3]\nb: [3, 2, 1]\n\na.color: #FF0000\na.label: Test\na.marker: *\n",
        )
        .unwrap()
        .svg;
        assert!(svg.contains("<path"));
        assert!(svg.contains("#FF0000"));
        assert!(svg.contains("Test"));
        assert!(svg.contains('*'));
    }


    /// Bars as (x, y, width, height), excluding the background and legend chips.
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
                continue; // legend chip
            }
            out.push((x, y, width, height));
        }
        out
    }

    fn texts(svg: &str) -> Vec<String> {
        svg.split("<text ")
            .skip(1)
            .filter_map(|part| {
                let open = part.find('>')?;
                let rest = &part[open + 1..];
                let close = rest.find("</text>")?;
                Some(rest[..close].to_string())
            })
            .collect()
    }

    #[test]
    fn bar_series_are_grouped_side_by_side() {
        let svg = render("bar\n\nq1: [12, 18, 9]\nq2: [8, 14, 19]\n")
            .unwrap()
            .svg;
        let bars = bars(&svg);
        assert_eq!(bars.len(), 6, "two series over three categories");
        let first_row: Vec<_> = bars.iter().take(2).collect();
        assert!(
            (first_row[0].0 - first_row[1].0).abs() > 1.0,
            "series overlap: {:?}",
            first_row
        );
        assert!((first_row[0].2 - first_row[1].2).abs() < 0.01);
        let labels = texts(&svg);
        assert!(labels.contains(&"1".to_string()));
        assert!(labels.contains(&"3".to_string()));
    }

    #[test]
    fn bar_category_labels_come_from_the_global_option() {
        let svg = render("bar\n\nlabels: [一月, 二月, 三月]\ns: [1, 2, 3]\n")
            .unwrap()
            .svg;
        let labels = texts(&svg);
        for expected in ["一月", "二月", "三月"] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing label {expected}: {labels:?}"
            );
        }
    }

    #[test]
    fn bar_long_labels_are_truncated_not_overlapped() {
        let svg = render(
            "bar\n\nlabels: [a-very-long-category-name-that-cannot-fit, second]\ns: [1, 2]\n",
        )
        .unwrap()
        .svg;
        let labels = texts(&svg);
        assert!(
            labels.iter().any(|label| label.ends_with('…')),
            "no truncated label: {labels:?}"
        );
        assert!(!labels.iter().any(|label| label.contains("cannot-fit")));
    }

    #[test]
    fn bar_negative_values_draw_below_the_zero_line() {
        let svg = render("bar\n\ns: [-5, 10]\n").unwrap().svg;
        let bars = bars(&svg);
        assert_eq!(bars.len(), 2);
        let zero_line = svg
            .split("<line ")
            .filter_map(|part| {
                if !part.contains("stroke-width=\"1.5\"") {
                    return None;
                }
                let start = part.find("y1=\"")? + 4;
                let rest = &part[start..];
                rest[..rest.find('"')?].parse::<f32>().ok()
            })
            .next()
            .expect("a zero baseline");
        // The negative bar grows downward from the baseline, the positive one up.
        let negative = bars.iter().find(|bar| bar.1 >= zero_line - 0.5).expect("negative bar");
        let positive = bars.iter().find(|bar| bar.1 < zero_line - 0.5).expect("positive bar");
        assert!(negative.1 + negative.3 > zero_line);
        assert!(positive.3 > 0.0);
    }

    #[test]
    fn bar_value_labels_are_optional() {
        let with = render("bar\n\nvalues: true\ns: [12, 18]\n").unwrap().svg;
        assert!(texts(&with).iter().any(|text| text == "12"));
        let without = render("bar\n\ns: [12, 18]\n").unwrap().svg;
        assert!(!texts(&without).iter().any(|text| text == "12"));
        let per_series = render("bar\n\ns: [7]\ns.values: true\n").unwrap().svg;
        assert!(texts(&per_series).iter().any(|text| text == "7"));
    }

    #[test]
    fn bar_horizontal_draws_wide_bars() {
        let svg = render("bar\n\nhorizontal: true\nlabels: [A, B]\ns: [3, 8]\n")
            .unwrap()
            .svg;
        let bars = bars(&svg);
        assert_eq!(bars.len(), 2);
        for bar in &bars {
            assert!(bar.2 > bar.3, "horizontal bar is not wide: {bar:?}");
        }
    }

    #[test]
    fn bar_stacking_places_segments_end_to_end() {
        let svg = render("bar\n\nstacked: true\ns1: [3]\ns2: [4]\n").unwrap().svg;
        let bars = bars(&svg);
        assert_eq!(bars.len(), 2);
        let (a, b) = (bars[0], bars[1]);
        // Same category column, adjoining vertical extents, no gap or overlap.
        assert!((a.0 - b.0).abs() < 0.01);
        let (upper, lower) = if a.1 < b.1 { (a, b) } else { (b, a) };
        assert!((upper.1 + upper.3 - lower.1).abs() < 0.5);
    }

    #[test]
    fn reserved_globals_win_over_series_names() {
        let svg = render("bar\n\nstacked: true\ns: [1]\n").unwrap().svg;
        assert!(!texts(&svg).iter().any(|text| text == "stacked"));
    }

    #[test]
    fn unknown_options_are_errors() {
        assert!(render("bar\n\ns: [1]\ns.colour: #ff0000\n").is_err());
        assert!(render("bar\n\ns: [1]\ns.values: maybe\n").is_ok());
    }

    #[test]
    fn line_and_pie_output_is_unchanged_by_the_bar_work() {
        let line = render("line\n\na: [1, 2, 3]\n").unwrap().svg;
        assert!(line.contains("<path"));
        assert!(!line.contains("text-anchor=\"middle\" font-family=\"Segoe UI, sans-serif\" font-size=\"11\" fill=\"#555\""));
        let pie = render("pie\n\na: 10%\nb: 30%\nc: 50%\n").unwrap().svg;
        assert!(pie.contains(UNDEFINED));
    }


    /// The reference document is the interface contract: every accepted option
    /// must be documented and every documented example must render.
    fn reference() -> &'static str {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/chart.md"))
    }

    /// Option names in the reference's option tables (sections 4 and 5).
    fn documented_options(doc: &str) -> Vec<(String, bool)> {
        let mut options = Vec::new();
        let mut in_tables = false;
        let mut global_section = false;
        for line in doc.lines() {
            if line.starts_with("## 4.") {
                in_tables = true;
                global_section = true;
                continue;
            }
            if line.starts_with("## 5.") {
                in_tables = true;
                global_section = false;
                continue;
            }
            if line.starts_with("## 6.") {
                break;
            }
            if !in_tables || !line.starts_with("| ") {
                continue;
            }
            let Some(first) = line.split('|').nth(1) else {
                continue;
            };
            let cell = first.trim();
            let cell = cell.trim_matches('`');
            if cell.is_empty() || cell == "选项" || cell.starts_with('-') {
                continue;
            }
            for name in cell.split('/') {
                let name = name.trim().trim_matches('`').trim();
                if !name.is_empty() && !name.contains('…') {
                    options.push((name.to_string(), global_section));
                }
            }
        }
        options
    }

    #[test]
    fn the_reference_documents_exactly_the_accepted_options() {
        let doc = reference();
        let documented = documented_options(doc);
        for (name, scope) in OPTIONS {
            assert!(
                documented
                    .iter()
                    .any(|(o, global)| o == name && *global == (*scope == "global")),
                "option {name} ({scope}) is accepted but not documented in its table"
            );
        }
        for (name, global) in &documented {
            assert!(
                OPTIONS.iter().any(|(o, scope)| {
                    *o == name.as_str() && *scope == if *global { "global" } else { "series" }
                }),
                "documented option {name} is not in the parser inventory"
            );
        }
    }

    #[test]
    fn every_documented_example_renders() {
        let doc = reference();
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
        assert!(current.is_none(), "unterminated chart example in docs/chart.md");
        assert!(examples.len() >= 10, "only {} examples found", examples.len());
        for (index, source) in examples.iter().enumerate() {
            render(source).unwrap_or_else(|error| {
                panic!("docs/chart.md example {index} does not render: {error}\n{source}")
            });
        }
    }

    #[test]
    fn invalid_chart_is_an_error() {
        assert!(render("nope").is_err());
    }
}
