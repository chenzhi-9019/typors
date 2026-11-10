//! RaTeX-backed math typesetting.
//!
//! The engine returns two things: the box metrics the layout needs before it can
//! place the formula, and (lazily) the self-contained SVG the painter draws. Both
//! are cached by `(tex, font_size)` because a document is re-parsed on every
//! keystroke but its formulas rarely change.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_svg::{SvgOptions, render_to_svg};

/// Default number of formulas kept in one engine instance.
pub const DEFAULT_CACHE_CAPACITY: usize = 512;

/// A user-facing math failure. Never a panic: bad formulas must degrade to source
/// text, not take the process down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathError {
    pub message: String,
}

impl MathError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MathError {}

/// The formula's box, in logical pixels at the requested font size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathMetrics {
    pub width_px: f32,
    /// Ascent above the baseline.
    pub height_px: f32,
    /// Descent below the baseline.
    pub depth_px: f32,
}

impl MathMetrics {
    pub fn total_height_px(self) -> f32 {
        self.height_px + self.depth_px
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct MathKey {
    tex: String,
    font_size_bits: u32,
}

struct CachedMath {
    metrics: Result<MathMetrics, MathError>,
    svg: OnceLock<Result<Arc<str>, MathError>>,
}

struct CacheState {
    entries: HashMap<MathKey, Arc<CachedMath>>,
    order: VecDeque<MathKey>,
    capacity: usize,
}

impl CacheState {
    fn insert(&mut self, key: MathKey, value: Arc<CachedMath>) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key, value);
            return;
        }
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
    }
}

/// RaTeX wrapper with a bounded `(tex, font_size)` cache.
pub struct MathEngine {
    cache: Mutex<CacheState>,
}

impl Default for MathEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MathEngine {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CACHE_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            cache: Mutex::new(CacheState {
                entries: HashMap::new(),
                order: VecDeque::new(),
                capacity: capacity.max(1),
            }),
        }
    }

    /// Number of cached formulas (test/diagnostic hook).
    pub fn len(&self) -> usize {
        self.cache.lock().map(|cache| cache.entries.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.entries.clear();
            cache.order.clear();
        }
    }

    /// Lay out `tex` and return the box metrics used by the document flow.
    pub fn measure(&self, tex: &str, font_size_px: f32) -> Result<MathMetrics, MathError> {
        let entry = self.entry(tex, font_size_px)?;
        entry.metrics.clone()
    }

    /// The self-contained SVG for `tex`. Generated on first call and then shared.
    pub fn svg(&self, tex: &str, font_size_px: f32) -> Result<Arc<str>, MathError> {
        let entry = self.entry(tex, font_size_px)?;
        entry
            .svg
            .get_or_init(|| build_svg(tex, font_size_px))
            .clone()
    }

    fn entry(&self, tex: &str, font_size_px: f32) -> Result<Arc<CachedMath>, MathError> {
        if !font_size_px.is_finite() || font_size_px <= 0.0 {
            return Err(MathError::new(format!(
                "invalid math font size: {font_size_px}"
            )));
        }
        let key = MathKey {
            tex: tex.to_string(),
            font_size_bits: font_size_px.to_bits(),
        };
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| MathError::new("math cache poisoned"))?;
        if let Some(entry) = cache.entries.get(&key) {
            return Ok(entry.clone());
        }
        let metrics = build_metrics(tex, font_size_px);
        let entry = Arc::new(CachedMath {
            metrics,
            svg: OnceLock::new(),
        });
        cache.insert(key, entry.clone());
        Ok(entry)
    }
}

fn build_metrics(tex: &str, font_size_px: f32) -> Result<MathMetrics, MathError> {
    let box_ = layout_box(tex)?;
    Ok(MathMetrics {
        width_px: finite_non_negative(box_.width * f64::from(font_size_px)),
        height_px: finite_non_negative(box_.height * f64::from(font_size_px)),
        depth_px: finite_non_negative(box_.depth * f64::from(font_size_px)),
    })
}

fn build_svg(tex: &str, font_size_px: f32) -> Result<Arc<str>, MathError> {
    let box_ = layout_box(tex)?;
    let list = to_display_list(&box_);
    let options = SvgOptions {
        font_size: f64::from(font_size_px),
        padding: 0.0,
        embed_glyphs: true,
        ..SvgOptions::default()
    };
    let svg = guard("svg export", || render_to_svg(&list, &options))?;
    if svg.contains("<text") {
        return Err(MathError::new(
            "math glyphs were not converted to vector paths",
        ));
    }
    Ok(Arc::from(svg))
}

fn layout_box(tex: &str) -> Result<ratex_layout::LayoutBox, MathError> {
    let nodes = guard("parse", || ratex_parser::parse(tex))?
        .map_err(|error| MathError::new(error.to_string()))?;
    guard("layout", || layout(&nodes, &LayoutOptions::default()))
}

/// RaTeX is expected to return errors, but a layout edge case must not unwind
/// the UI thread: wrap the call and turn a panic into a formula error.
fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, MathError> {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match caught {
        Ok(value) => Ok(value),
        Err(payload) => {
            let message = if let Some(text) = payload.downcast_ref::<&str>() {
                (*text).to_string()
            } else if let Some(text) = payload.downcast_ref::<String>() {
                text.clone()
            } else {
                "unknown panic".to_string()
            };
            Err(MathError::new(format!("math {what} panicked: {message}")))
        }
    }
}

fn finite_non_negative(value: f64) -> f32 {
    if value.is_finite() {
        value.max(0.0) as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_formulas_measure_and_export_paths() {
        let engine = MathEngine::new();
        for tex in [
            "E = mc^2",
            "\\frac{a}{b} + \\sqrt{x}",
            "\\sum_{i=0}^{n} i",
            "\\int_0^\\infty e^{-x^2}\\,dx",
            "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
            "\\text{你好} + \\alpha",
        ] {
            let metrics = engine.measure(tex, 20.0).unwrap_or_else(|e| panic!("{tex}: {e}"));
            assert!(metrics.width_px > 0.0, "{tex}: {metrics:?}");
            assert!(metrics.total_height_px() > 0.0, "{tex}: {metrics:?}");
            let svg = engine.svg(tex, 20.0).expect("svg");
            assert!(svg.starts_with("<svg"), "{tex}: {}", &svg[..80.min(svg.len())]);
            assert!(svg.contains("<path"), "{tex}: no glyph paths");
            assert!(!svg.contains("<text"), "{tex}: text elements survive");
        }
    }

    #[test]
    fn invalid_formulas_return_errors_and_do_not_panic() {
        let engine = MathEngine::new();
        let error = engine.measure("\\badcommand{x}", 16.0).unwrap_err();
        assert!(error.message.contains("Undefined control sequence"), "{error}");
        assert!(engine.svg("\\badcommand{x}", 16.0).is_err());
        assert!(engine.measure("x", 0.0).is_err());
        assert!(engine.measure("x", f32::NAN).is_err());
    }

    #[test]
    fn degenerate_formulas_do_not_panic() {
        let engine = MathEngine::new();
        for tex in ["", "   ", "\\frac{}{}", "\\sqrt{}"] {
            let _ = engine.measure(tex, 16.0);
            let _ = engine.svg(tex, 16.0);
        }
    }

    #[test]
    fn cache_is_bounded_and_reuses_the_svg() {
        let engine = MathEngine::with_capacity(2);
        engine.measure("a", 10.0).unwrap();
        engine.measure("b", 10.0).unwrap();
        let first = engine.svg("a", 10.0).unwrap();
        let second = engine.svg("a", 10.0).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        engine.measure("c", 10.0).unwrap();
        assert_eq!(engine.len(), 2);
    }

    #[test]
    fn metrics_scale_with_font_size() {
        let engine = MathEngine::new();
        let small = engine.measure("x^2", 10.0).unwrap();
        let large = engine.measure("x^2", 20.0).unwrap();
        assert!((large.width_px - small.width_px * 2.0).abs() < 0.01);
        assert!((large.height_px - small.height_px * 2.0).abs() < 0.01);
    }
}
