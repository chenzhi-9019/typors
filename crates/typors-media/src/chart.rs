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
}

#[derive(Clone, Debug)]
struct Spec {
    kind: ChartKind,
    series: BTreeMap<String, Series>,
    title: Option<String>,
    width: f32,
    height: f32,
}

const DEFAULT_COLORS: &[&str] = &[
    "#4E79A7", "#F28E2B", "#E15759", "#76B7B2", "#59A14F", "#EDC948", "#B07AA1", "#FF9DA7",
    "#9C755F", "#BAB0AC",
];
const UNDEFINED: &str = "#B0B0B0";

pub fn render(source: &str) -> Result<Chart, String> {
    let spec = parse(source)?;
    let svg = match spec.kind {
        ChartKind::Pie => pie(&spec),
        ChartKind::Line | ChartKind::Bar | ChartKind::Scatter => cartesian(&spec),
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
    };
    for (index, line) in lines.enumerate() {
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("chart line {}: expected `key: value`", index + 2));
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
            "line" => series.line = value.eq_ignore_ascii_case("true") || value == "1",
            "x" | "labels" => {
                series.labels = value
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|part| string_value(part.trim()))
                    .collect();
            }
            _ => {}
        }
    }
    if spec.series.is_empty() {
        return Err("chart: no series defined".to_string());
    }
    Ok(spec)
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

    #[test]
    fn invalid_chart_is_an_error() {
        assert!(render("nope").is_err());
    }
}
