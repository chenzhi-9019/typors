//! Render-projection layout: the second projection of the single buffer.
//!
//! Everything here is geometry only. [`DocLayout`] decides which fragments are
//! visible, where each one sits and how tall a block is; text measurement is
//! abstracted behind [`Metrics`] so that
//!
//! * the real painter measures through the window text system ([`WindowMetrics`],
//!   i.e. exactly the `ShapedLine::width()` that gets painted), and
//! * `--dump-layout` can rebuild the same geometry without a window
//!   ([`ApproxMetrics`]) when no surface is available.
//!
//! Group 4 (caret, click hit testing) must come through this module rather than
//! doing its own arithmetic: [`DocLayout::hit`], [`DocLayout::block_at_point`]
//! and [`DocLayout::caret_bounds`] are the only places that know how a byte
//! offset inside `Block::render` turns into an x coordinate.
//!
//! Two invariants matter for the "no Markdown markers on screen" claim, and both
//! are checkable from a [`DocLayout`] alone (see [`DocLayout::checks`]):
//!
//! 1. A span is painted only when `span.is_visible()`; `SpanKind::Marker` spans
//!    have an empty `render` range, so they never reach the painter and never
//!    advance the pen.
//! 2. Concatenating the painted fragments of a block reproduces
//!    `block.render` with its newlines removed •nothing dropped, nothing doubled.

use std::{
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use gpui_kit::gpui::*;

use typors_core::MarkedText;
use typors_core::doc::{self, Block, BlockKind, Document, ObjectKind, SpanKind};

/// Upper bound on the `A4` geometry probes: each probe re-shapes a fragment, so a
/// 100k-character document would otherwise spend minutes in the check.
const A4_MAX_PROBES: usize = 4_000;

/// Ceiling on cached shaped fragments. A 100k-character document holds ~7.7k
/// fragments and needs up to two entries each (geometry + paint colour); the cap
/// is only there so a long editing session cannot grow the cache without bound.
const SHAPE_CACHE_LIMIT: usize = 40_000;

// ---------------------------------------------------------------- shape cache

/// Identity of one shaped fragment.
///
/// Geometry does not depend on colour, but a [`ShapedLine`] bakes its run colour
/// in (`DecorationRun`), so a paintable line and a measurable line are two
/// entries: the layout pass asks for black/no-underline, `prepaint` asks for the
/// fragment's real colour.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    font: u64,
    text: Box<str>,
    font_size: u32,
    style: [bool; 4],
    force_bold: bool,
    color: u32,
    underline: bool,
}

/// Cross-frame cache of shaped fragments (task 7.1, optimisation ①).
///
/// `layout` measures every fragment and `prepaint` shapes every fragment again;
/// on a 100k-character document that is ~15k `shape_line` calls and 70% of the
/// frame. Between two frames almost all of them are the same text in the same
/// font, so the work is done once and looked up afterwards.
///
/// A hit returns a clone, which is cheap: `ShapedLine` holds the glyph run behind
/// an `Arc` (`text_system/line.rs:43-54`).
#[derive(Default)]
pub struct ShapeCache {
    entries: Mutex<HashMap<ShapeKey, ShapedLine>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl fmt::Debug for ShapeCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (hits, misses) = self.stats();
        f.debug_struct("ShapeCache")
            .field("entries", &self.len())
            .field("hits", &hits)
            .field("misses", &misses)
            .finish()
    }
}

impl ShapeCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// `(hits, misses)` since the cache was created. Reported by `--dump-perf`.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|map| map.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop everything (an edit invalidates nothing here — the keys are the
    /// fragment text itself — but a caller may want the cold cost back).
    pub fn clear(&self) {
        if let Ok(mut map) = self.entries.lock() {
            map.clear();
        }
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }

    fn get_or_shape(&self, key: ShapeKey, shape: impl FnOnce() -> ShapedLine) -> ShapedLine {
        if let Ok(map) = self.entries.lock()
            && let Some(line) = map.get(&key)
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return line.clone();
        }
        let line = shape();
        self.misses.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut map) = self.entries.lock() {
            if map.len() >= SHAPE_CACHE_LIMIT {
                map.clear();
            }
            map.insert(key, line.clone());
        }
        line
    }
}

// ---------------------------------------------------------------- metrics

/// What the layout needs to know about a font, without owning a `Window`.
///
/// `text` is always a single line (no `\n`): [`DocLayout`] splits on newlines
/// before it ever measures anything, because `shape_line` debug-asserts on
/// embedded newlines (`text_system.rs:645-648`).
pub trait Metrics: fmt::Debug + Send + Sync + 'static {
    /// Short label for reports: which implementation produced these numbers.
    fn label(&self) -> &'static str;
    /// Identity of the font this instance measures with, for caches that must
    /// not survive a font change. Defaults to the label, which is enough for the
    /// font-free [`ApproxMetrics`].
    fn tag(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.label().hash(&mut hasher);
        hasher.finish()
    }
    /// Advance width of one line of text.
    fn advance(&self, text: &str, font_size: Pixels, style: doc::Style) -> Pixels;
    /// x offset of `index` (a UTF-8 byte offset inside `text`).
    fn x_for_index(&self, text: &str, font_size: Pixels, style: doc::Style, index: usize)
    -> Pixels;
    /// Nearest byte offset to `x` inside `text`.
    fn index_for_x(&self, text: &str, font_size: Pixels, style: doc::Style, x: Pixels) -> usize;
    /// Distance from the line box top to the baseline. Objects use this to align
    /// their own baseline with the surrounding text.
    fn ascent(&self, font_size: Pixels, style: doc::Style) -> Pixels {
        let _ = style;
        px(font_size.as_f32() * 0.8)
    }
}

/// The font a span is drawn with. Heading bold and code monospace are forced by
/// the block, inline emphasis/monospace come from the span itself.
pub fn font_for(
    base: &Font,
    mono_family: &SharedString,
    style: doc::Style,
    force_bold: bool,
) -> Font {
    let mut font = base.clone();
    if style.code {
        font.family = mono_family.clone();
    } else if style.bold || force_bold {
        font = font.bold();
    }
    if style.bold || force_bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font = font.italic();
    }
    font
}

/// Real metrics: shapes through the window's text system, so the numbers are
/// identical to what `ShapedLine::paint` puts on screen.
///
/// Since task 7.1 it also owns a [`ShapeCache`] shared with the frame that
/// painted last, so both the measuring pass here and `prepaint`'s shaping pass
/// become lookups once a fragment has been shaped in this font.
pub struct WindowMetrics {
    text_system: Arc<WindowTextSystem>,
    base_font: Font,
    mono_family: SharedString,
    cache: Arc<ShapeCache>,
    /// Hash of the font pair every fragment's font is derived from
    /// ([`font_for`]); part of the cache key so a theme/font change cannot hit a
    /// line shaped in the old one.
    font_tag: u64,
}

impl fmt::Debug for WindowMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WindowMetrics")
    }
}

// Background measurement (task 5) moves an `Arc<dyn Metrics>` onto a worker
// thread. If this assertion ever fails, the scheduler must run its chunks
// cooperatively on the main thread instead; the public behaviour is the same.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WindowMetrics>();
};

impl WindowMetrics {
    pub fn new(window: &Window, mono_family: SharedString) -> Self {
        Self::with_cache(window, mono_family, Arc::new(ShapeCache::new()))
    }

    /// Metrics that share `cache` with the previous frame (task 7.1).
    pub fn with_cache(window: &Window, mono_family: SharedString, cache: Arc<ShapeCache>) -> Self {
        let base_font = window.text_style().font();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        base_font.hash(&mut hasher);
        mono_family.hash(&mut hasher);
        let font_tag = hasher.finish();
        Self {
            text_system: window.text_system().clone(),
            base_font,
            mono_family,
            cache,
            font_tag,
        }
    }

    /// The cache this instance reads and fills.
    pub fn cache(&self) -> &Arc<ShapeCache> {
        &self.cache
    }

    /// Shape one single-line fragment exactly as it will be painted.
    pub fn shaped(
        &self,
        text: &str,
        font_size: Pixels,
        style: doc::Style,
        force_bold: bool,
        color: Rgba,
        underline: bool,
    ) -> ShapedLine {
        let key = ShapeKey {
            font: self.font_tag,
            text: text.into(),
            font_size: font_size.as_f32().to_bits(),
            style: [style.bold, style.italic, style.code, style.link],
            force_bold,
            color: u32::from(color),
            underline,
        };
        self.cache.get_or_shape(key, || {
            let font = font_for(&self.base_font, &self.mono_family, style, force_bold);
            let run = TextRun {
                len: text.len(),
                font,
                color: Hsla::from(color),
                background_color: None,
                underline: underline.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(Hsla::from(color)),
                    wavy: false,
                }),
                strikethrough: None,
            };
            self.text_system
                .shape_line(text.into(), font_size, std::slice::from_ref(&run), None)
        })
    }
}

impl Metrics for WindowMetrics {
    fn label(&self) -> &'static str {
        "window-text-system"
    }

    fn tag(&self) -> u64 {
        self.font_tag
    }

    fn advance(&self, text: &str, font_size: Pixels, style: doc::Style) -> Pixels {
        self.shaped(text, font_size, style, false, rgba(0x000000FF), false)
            .width()
    }

    fn x_for_index(
        &self,
        text: &str,
        font_size: Pixels,
        style: doc::Style,
        index: usize,
    ) -> Pixels {
        let line = self.shaped(text, font_size, style, false, rgba(0x000000FF), false);
        line.x_for_index(index.min(text.len()))
    }

    fn index_for_x(&self, text: &str, font_size: Pixels, style: doc::Style, x: Pixels) -> usize {
        let line = self.shaped(text, font_size, style, false, rgba(0x000000FF), false);
        if x.as_f32() <= 0. {
            return 0;
        }
        if x >= line.width() {
            return text.len();
        }
        line.closest_index_for_x(x).min(text.len())
    }

    fn ascent(&self, font_size: Pixels, style: doc::Style) -> Pixels {
        self.shaped("M", font_size, style, false, rgba(0x000000FF), false)
            .ascent
    }
}

/// Deterministic stand-in metrics for a headless run.
///
/// Not glyph-accurate by design: it exists so `--dump-layout` still produces
/// geometry, span order and the marker-free assertions when the window cannot be
/// created. Every report states which implementation measured it.
#[derive(Debug, Default, Clone, Copy)]
pub struct ApproxMetrics;

impl ApproxMetrics {
    fn em(ch: char) -> f32 {
        match ch as u32 {
            0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD => 1.0,
            0x0300..=0x036F => 0.0, // combining marks
            0x200D => 0.0,          // ZWJ
            _ => 0.55,
        }
    }

    fn width_f32(text: &str, font_size: Pixels, style: doc::Style) -> f32 {
        let scale = if style.code {
            0.62
        } else if style.bold {
            1.04
        } else {
            1.0
        };
        text.chars().map(Self::em).sum::<f32>() * font_size.as_f32() * scale
    }
}

impl Metrics for ApproxMetrics {
    fn label(&self) -> &'static str {
        "approx"
    }

    fn advance(&self, text: &str, font_size: Pixels, style: doc::Style) -> Pixels {
        px(Self::width_f32(text, font_size, style))
    }

    fn x_for_index(
        &self,
        text: &str,
        font_size: Pixels,
        style: doc::Style,
        index: usize,
    ) -> Pixels {
        let index = index.min(text.len());
        px(Self::width_f32(&text[..index], font_size, style))
    }

    fn index_for_x(&self, text: &str, font_size: Pixels, style: doc::Style, x: Pixels) -> usize {
        let mut pen = 0.;
        for (offset, ch) in text.char_indices() {
            let w = Self::em(ch) * font_size.as_f32() * if style.code { 0.62 } else { 1.0 };
            if x.as_f32() < pen + w / 2. {
                return offset;
            }
            pen += w;
        }
        text.len()
    }
}

// ---------------------------------------------------------------- theme

/// Visual constants. Deliberately our own: no Typora theme, CSS or font asset is
/// read or reused. Values were picked to be readable, not to match Typora.
#[derive(Clone, Debug)]
pub struct EditorTheme {
    pub body_size: f32,
    pub line_ratio: f32,
    pub page_pad: f32,
    /// Typora `#write` max width (860/1024/1200 at the CSS breakpoints).
    pub page_width: f32,
    pub block_gap: f32,
    pub heading_gap: f32,
    pub heading_scale: [f32; 6],
    pub heading_line_ratio: f32,
    pub code_ratio: f32,
    pub code_pad: f32,
    pub indent_per_level: f32,
    pub bullet_col: f32,
    pub quote_indent: f32,
    pub quote_bar_w: f32,
    pub rule_thickness: f32,
    pub mono_family: SharedString,
    pub page_bg: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub code_text: Rgba,
    pub code_bg: Rgba,
    pub link: Rgba,
    pub rule: Rgba,
    pub quote_bar: Rgba,
    /// Caret bar (#1F2328 at full alpha is the body text colour).
    pub caret: Rgba,
    /// Selection highlight. Painted *under* the text, so a selection never hides
    /// the glyphs it covers.
    pub selection: Rgba,
}

impl EditorTheme {
    /// Typora's light/default tokens plus the persisted appearance overrides.
    pub fn from_settings(settings: &typors_settings::Settings) -> Self {
        let mut theme = Self::default();
        theme.body_size = settings.appearance.font_size;
        theme.line_ratio = settings.appearance.line_height;
        theme.page_width = settings.appearance.page_width;
        if settings.appearance.theme == typors_settings::ThemeChoice::Dark {
            theme = theme.dark();
        }
        theme
    }

    /// Typora `night.css` palette; typography stays from the current theme.
    pub fn dark(mut self) -> Self {
        self.page_bg = rgb(0x363B40);
        self.text = rgb(0xDEDEDE);
        self.muted = rgb(0x9E9E9E);
        self.code_text = rgb(0xDEDEDE);
        self.code_bg = rgb(0x2F3438);
        self.link = rgb(0x6FB3E0);
        self.rule = rgb(0x4A4F55);
        self.quote_bar = rgb(0x5A6066);
        self.caret = rgb(0xDEDEDE);
        self.selection = rgb(0x4A6B8A);
        self
    }
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self {
            body_size: 16.,
            line_ratio: 1.6,
            page_pad: 30.,
            page_width: 860.,
            block_gap: 12.8,
            heading_gap: 16.,
            heading_scale: [2.0, 1.8, 1.6, 1.4, 1.2, 1.0],
            heading_line_ratio: 1.4,
            code_ratio: 0.9,
            code_pad: 8.,
            indent_per_level: 30.,
            bullet_col: 30.,
            quote_indent: 15.,
            quote_bar_w: 4.,
            rule_thickness: 2.,
            mono_family: "Consolas".into(),
            page_bg: rgb(0xFFFFFF),
            text: rgb(0x333333),
            muted: rgb(0x777777),
            code_text: rgb(0x333333),
            code_bg: rgb(0xF8F8F8),
            link: rgb(0x4183C4),
            rule: rgb(0xE7E7E7),
            quote_bar: rgb(0xDFE2E5),
            caret: rgb(0x333333),
            selection: rgb(0xB5D6FC),
        }
    }
}

// ---------------------------------------------------------------- geometry

/// The box a rich-content object occupies inside its line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectMetrics {
    pub width: Pixels,
    pub height: Pixels,
    /// Ascent above the baseline (used to align inline objects with text).
    pub ascent: Pixels,
    /// Descent below the baseline.
    pub descent: Pixels,
    /// `$$...$$` objects get their own centred line.
    pub display: bool,
}

/// Resolves an [`ObjectKind`] to box metrics. The editor implements this with the
/// image registry and the RaTeX engine; headless layouts use [`NoMedia`].
pub trait MediaMeasures {
    fn measure(&self, kind: &ObjectKind, font_size: Pixels) -> ObjectMetrics;
}

/// Deterministic placeholders for layout tests and headless probes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoMedia;

impl MediaMeasures for NoMedia {
    fn measure(&self, kind: &ObjectKind, font_size: Pixels) -> ObjectMetrics {
        let _ = font_size;
        match kind {
            ObjectKind::Image { .. } => ObjectMetrics {
                width: px(320.),
                height: px(180.),
                ascent: px(135.),
                descent: px(45.),
                display: false,
            },
            ObjectKind::Math { display, .. } => ObjectMetrics {
                width: if *display { px(240.) } else { px(80.) },
                height: if *display { px(48.) } else { px(22.) },
                ascent: if *display { px(36.) } else { px(17.) },
                descent: if *display { px(12.) } else { px(5.) },
                display: *display,
            },
            ObjectKind::Diagram { .. } => ObjectMetrics {
                width: px(480.),
                height: px(240.),
                ascent: px(180.),
                descent: px(60.),
                display: true,
            },
            ObjectKind::Chart { .. } => ObjectMetrics {
                width: px(560.),
                height: px(340.),
                ascent: px(255.),
                descent: px(85.),
                display: true,
            },
        }
    }
}

/// One object placed in a block. Coordinates are block-relative until
/// [`BlockBox`] materialization adds the block top.
#[derive(Clone, Debug)]
pub struct ObjectBox {
    /// Index into `Block::spans`.
    pub span_idx: usize,
    pub kind: ObjectKind,
    /// Placeholder byte range inside `Block::render`.
    pub render: Range<usize>,
    /// Absolute source range (the complete Markdown syntax).
    pub source: Range<usize>,
    /// Font size the object was laid out at (used by the math engine).
    pub font_size: Pixels,
    pub bounds: Bounds<Pixels>,
    /// Baseline y in the same coordinate space as `bounds`.
    pub baseline: Pixels,
    pub display: bool,
}

/// One paintable fragment: a single line of a single span.
#[derive(Clone, Debug)]
pub struct DrawSpan {
    pub block_idx: usize,
    /// Index into `Block::spans`; always a visible span.
    pub span_idx: usize,
    /// The text of this fragment (never contains `\n`).
    pub text: String,
    /// Byte range of this fragment inside `Block::render`.
    pub render: Range<usize>,
    pub style: doc::Style,
    pub font_size: Pixels,
    pub line_height: Pixels,
    /// Top-left of the fragment's line box.
    pub origin: Point<Pixels>,
    pub width: Pixels,
    pub color: Rgba,
    pub underline: bool,
    pub strike: bool,
    pub force_bold: bool,
}

/// Decoration owned by the renderer rather than by the source: list bullets and
/// ordinals, the code-block plate, quote bars and rules.
#[derive(Clone, Debug)]
pub enum BlockDecor {
    Quad {
        bounds: Bounds<Pixels>,
        color: Rgba,
    },
    Text {
        text: String,
        origin: Point<Pixels>,
        font_size: Pixels,
        line_height: Pixels,
        color: Rgba,
    },
}

#[derive(Clone, Debug)]
pub struct BlockBox {
    pub block_idx: usize,
    pub kind: BlockKind,
    pub src: Range<usize>,
    pub bounds: Bounds<Pixels>,
    /// `Block::render` copied for hit testing (char-boundary snapping).
    pub render: String,
    /// The projection this block was laid out in.
    pub mode: BlockMode,
    /// Raw Markdown source of the block, present exactly when `mode` is
    /// [`BlockMode::Source`].
    pub source_text: Option<String>,
    /// Byte range of the IME composition inside [`BlockBox::display_text`], when
    /// this block is showing one (`crate::ime`, design D4).
    ///
    /// The composition is spliced into `source_text` for painting but never
    /// reaches the rope, so this range is display-space: `display_byte - len`
    /// turns an offset after it back into a buffer offset. See
    /// [`DocLayout::display_byte_for_src`].
    pub marked: Option<Range<usize>>,
    /// Left edge of the text column, with list indentation, code padding and
    /// quote bars already applied: where the caret sits on an empty line.
    pub content_left: Pixels,
    /// One line box of this block (a block has a single line height).
    pub line_height: Pixels,
    pub spans: Vec<DrawSpan>,
    pub decors: Vec<BlockDecor>,
    /// Atomic images/formulas painted as their own boxes, in the same coordinate
    /// space as `spans`.
    pub objects: Vec<ObjectBox>,
    /// False for a windowed-layout placeholder: its geometry is an estimate and
    /// it has no paint commands. A block that is visible in a frame is always
    /// measured before that frame paints.
    pub measured: bool,
}

/// One visual line of a block, exposed for caret and hit testing.
#[derive(Clone, Debug)]
pub struct LineBox {
    pub top: Pixels,
    pub bottom: Pixels,
    pub left: Pixels,
    pub right: Pixels,
    pub height: Pixels,
    /// Byte range inside the block's display text, excluding the newline.
    pub render: Range<usize>,
}

impl BlockBox {
    /// The text this block paints: the render projection, or its raw source.
    pub fn display_text(&self) -> &str {
        match &self.source_text {
            Some(text) => text,
            None => &self.render,
        }
    }

    /// The block's painted text: draw spans plus one placeholder per object, in
    /// render order. This is what the A1/`projection_invariants` checks compare
    /// against [`BlockBox::display_text`].
    pub fn painted_text(&self) -> String {
        let mut parts: Vec<(usize, &str)> = self
            .spans
            .iter()
            .map(|span| (span.render.start, span.text.as_str()))
            .chain(
                self.objects
                    .iter()
                    .map(|object| (object.render.start, "\u{FFFC}")),
            )
            .collect();
        parts.sort_by_key(|(start, _)| *start);
        parts.into_iter().map(|(_, text)| text).collect()
    }
}

/// Concatenate a block's painted fragments and object placeholders in render
/// order. Objects have no `DrawSpan`, so the check has to merge the two lists.
fn painted_text(block: &BlockBox) -> String {
    block.painted_text()
}

/// The whole document's render projection.
#[derive(Clone)]
pub struct DocLayout {
    /// Document origin; `blocks[*].bounds` are absolute.
    pub origin: Point<Pixels>,
    pub size: Size<Pixels>,
    pub content_width: Pixels,
    /// Which block was projected as source for this layout, if any.
    pub mode: LayoutMode,
    pub blocks: Vec<BlockBox>,
    /// The IME composition this layout painted, if one was live. Present means
    /// exactly one block carries [`BlockBox::marked`], and its painted text holds
    /// a string that is not in the buffer (design D4).
    pub marked: Option<MarkedText>,
    /// Which [`Metrics`] implementation produced the numbers.
    pub metrics_label: &'static str,
    pub metrics: Arc<dyn Metrics>,
    pub theme: EditorTheme,
    /// Wall time spent in [`DocLayout::layout`] (measuring included). H1 datum.
    pub measure_ms: f64,
    /// Wall time spent shaping the fragments for painting. H1 datum.
    pub shape_ms: f64,
    /// Fragments `prepaint` shaped for painting, and fragments the viewport cull
    /// dropped (task 7.1). Both are paint-time statistics: `blocks` above is
    /// always the complete document.
    pub shaped_fragments: usize,
    pub culled_fragments: usize,
    /// Slots whose geometry is exact and paintable.
    pub measured_blocks: usize,
    /// Slots carrying an estimated height because they are outside the measured
    /// window (the viewport plus margin).
    pub estimated_blocks: usize,
    /// The list ordinal of every block, recorded while the document is walked so
    /// a later windowed measurement cannot renumber a list.
    ordinals: Vec<Option<usize>>,
    /// The window this layout measured, or None for a whole-document layout.
    pub window: Option<Viewport>,
}

/// A click that landed on an atomic image/formula.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectHit {
    pub block_idx: usize,
    pub span_idx: usize,
    /// Complete Markdown source range of the object.
    pub source: Range<usize>,
    /// Which half of the box was clicked; the view puts the caret on the
    /// matching source edge.
    pub right_half: bool,
}

/// A resolved position: block, byte offset inside the block's *painted* text
/// ([`BlockBox::display_text`]: `Block::render` for a rendered block, the raw
/// Markdown source for a source-mode block), and which fragment owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderHit {
    pub block_idx: usize,
    pub render_byte: usize,
    pub span_idx: usize,
}

/// The projection rule for one frame: **at most one** block is drawn as Markdown
/// source (the block that owns the caret); every other block stays rendered.
///
/// This is the whole of "光标行源码还原" as a data structure: there is no way to
/// express two source blocks, so the spec's "其余部分始终维持渲染态" cannot be
/// violated by a caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutMode {
    pub source_block: Option<usize>,
}

impl LayoutMode {
    /// No caret: the whole document in the render projection.
    pub const RENDERED: LayoutMode = LayoutMode { source_block: None };

    pub fn source(block_idx: usize) -> Self {
        LayoutMode {
            source_block: Some(block_idx),
        }
    }

    pub fn is_source(&self, block_idx: usize) -> bool {
        self.source_block == Some(block_idx)
    }
}

/// How one block was projected in a given [`DocLayout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockMode {
    /// Marker-stripped text styled by the block's span table.
    Rendered,
    /// The block's raw Markdown source, one plain style.
    Source,
}

/// `DrawSpan::span_idx` for fragments of a source-mode block: they come from the
/// buffer, not from the span table, so no `Block::spans` index owns them.
pub const SOURCE_SPAN: usize = usize::MAX;

// ---------------------------------------------------------------- viewport

/// The vertical slice of the document that a frame actually puts on screen, in
/// the layout's own (element-local) coordinates.
///
/// Culling is a **paint-time** filter only (task 7.1): [`DocLayout`] always keeps
/// every block's exact geometry, because [`DocLayout::blocks`] is indexed by
/// `block_idx` and `display_byte_for_src`, `caret_x`, `line_tops`,
/// `selection::display_ranges`, `highlight_rects` and the probes all rely on that.
/// A culled frame therefore differs from a full one only in how many quads it
/// emits and how many fragments it shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub top: f32,
    pub bottom: f32,
}

impl Viewport {
    /// Everything: what a headless dump uses, and what a document shorter than the
    /// window gets.
    pub const UNBOUNDED: Viewport = Viewport {
        top: f32::NEG_INFINITY,
        bottom: f32::INFINITY,
    };

    pub fn new(top: f32, bottom: f32) -> Self {
        Self {
            top,
            bottom: bottom.max(top),
        }
    }

    /// The band a scrolled element actually shows.
    ///
    /// `element_top` is where the element's own origin lands in window
    /// coordinates — negative once the scroll container has moved the document
    /// up — so the visible band in element-local coordinates is
    /// `-element_top .. -element_top + window_height`. `margin` is the slack kept
    /// on both sides so a fragment straddling the edge (or one frame of
    /// sub-pixel scroll) is still shaped.
    pub fn on_screen(element_top: f32, window_height: f32, margin: f32) -> Self {
        let top = -element_top - margin;
        Self::new(top, top + window_height + margin * 2.)
    }

    pub fn is_unbounded(&self) -> bool {
        !self.top.is_finite() && !self.bottom.is_finite()
    }

    /// Whether the band `top..bottom` (a line box or a quad) is on screen.
    pub fn intersects(&self, top: f32, bottom: f32) -> bool {
        bottom >= self.top && top <= self.bottom
    }
}

// ---------------------------------------------------------------- block cache

/// Identity of one block's laid-out geometry (task 7.1, 块级缓存).
///
/// The geometry of a block depends only on its own text, the projection it is
/// drawn in, the list ordinal, the fonts and the page column — never on where it
/// sits vertically, which is why a cached block is stored with its top at `y = 0`
/// and re-used by translation. Font size, line height, column and colours are all
/// derived from the block kind and the theme, and both are covered by
/// [`CacheStamp`], so they are not repeated here.
///
/// `render` is identified by pointer + length rather than by hashing 196 KB of
/// text every frame; that is exact because the cache is dropped whenever
/// [`Document::revision`] or the document length changes, and within one revision
/// the block table (and therefore those strings) is untouched.
#[derive(Clone, PartialEq, Eq, Hash)]
struct BlockKey {
    kind: [u64; 2],
    render: (usize, usize),
    /// `(hash, len)` of the block's Markdown source, present only for the one
    /// block in the source projection.
    source: Option<(u64, usize)>,
    /// `(start, end, hash)` of a live IME composition inside that source.
    marked: Option<(usize, usize, u64)>,
    ordinal: Option<usize>,
    /// Bumped whenever an image's intrinsic size becomes known or changes.
    media: u64,
}

/// Everything that invalidates the whole block cache at once.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheStamp {
    revision: u64,
    len_bytes: usize,
    width: u32,
    origin_x: u32,
    font: u64,
    theme: u64,
    /// Image-loading generation; any change invalidates every block geometry.
    media: u64,
}

#[derive(Clone)]
struct BlockGeometry {
    spans: Vec<DrawSpan>,
    objects: Vec<ObjectBox>,
    decors: Vec<BlockDecor>,
    box_left: Pixels,
    box_right: Pixels,
    height: Pixels,
    content_left: Pixels,
    line_height: Pixels,
}

/// Per-block geometry cache: the "块级缓存" half of task 7.1.
///
/// A frame that changes nothing about a block (every scroll frame, and every
/// block except the two the caret touches when it moves) re-uses its laid-out
/// fragments instead of re-running [`Flow`]: no wrapping, no per-fragment
/// `advance`, no per-segment `String`. The cached geometry is relative to the
/// block's own top, so re-use is a translation.
///
/// Cost, stated plainly: the cache holds a second copy of the laid-out fragments
/// (a 100k-character document is ~7.7k `DrawSpan`s, a few MB). Cache hits are
/// counted and reported by `--dump-perf`.
#[derive(Default)]
pub struct BlockCache {
    stamp: Option<CacheStamp>,
    blocks: HashMap<usize, (BlockKey, BlockGeometry)>,
    hits: u64,
    misses: u64,
}

impl fmt::Debug for BlockCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockCache")
            .field("blocks", &self.blocks.len())
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .finish()
    }
}

impl BlockCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// `(hits, misses)` since the last invalidation.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.stamp = None;
        self.hits = 0;
        self.misses = 0;
    }

    /// Drop everything when the buffer, the page column, the font or the theme
    /// changed; entries are only valid inside one such stamp.
    fn prepare(&mut self, stamp: CacheStamp) -> bool {
        if self.stamp.as_ref() == Some(&stamp) {
            return true;
        }
        self.clear();
        self.stamp = Some(stamp);
        false
    }

    fn get(&mut self, block_idx: usize, key: &BlockKey) -> Option<BlockGeometry> {
        match self.blocks.get(&block_idx) {
            Some((cached, geometry)) if cached == key => {
                self.hits += 1;
                Some(geometry.clone())
            }
            _ => {
                self.misses += 1;
                None
            }
        }
    }

    fn put(&mut self, block_idx: usize, key: BlockKey, geometry: BlockGeometry) {
        self.blocks.insert(block_idx, (key, geometry));
    }
}

fn hash_str(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn theme_tag(theme: &EditorTheme) -> u64 {
    // Once per layout call, so formatting the theme is cheaper than hashing 28
    // fields by hand and cannot miss one that a later group adds.
    hash_str(&format!("{theme:?}"))
}

fn kind_tag(kind: &BlockKind) -> [u64; 2] {
    match kind {
        BlockKind::Heading { level } => [1, *level as u64],
        BlockKind::Paragraph => [2, 0],
        BlockKind::ListItem { ordered, level } => [3, u64::from(*ordered) | ((*level as u64) << 8)],
        BlockKind::FencedCode { info } => [4, hash_str(info)],
        BlockKind::IndentedCode => [5, 0],
        BlockKind::BlockQuote => [6, 0],
        BlockKind::Rule => [7, 0],
        BlockKind::Html => [8, 0],
        BlockKind::Table => [10, 0],
        BlockKind::Gap => [9, 0],
    }
}

impl fmt::Debug for DocLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocLayout")
            .field("origin", &self.origin)
            .field("size", &self.size)
            .field("blocks", &self.blocks.len())
            .field("metrics", &self.metrics_label)
            .finish()
    }
}

impl DocLayout {
    /// Lay the document out in the render projection (`LayoutMode::RENDERED`).
    pub fn layout(
        doc: &Document,
        theme: &EditorTheme,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
    ) -> Self {
        Self::layout_mode(doc, theme, metrics, origin, width, LayoutMode::RENDERED)
    }

    /// Lay the document out, projecting `mode.source_block` as Markdown source.
    ///
    /// The only difference between the two projections is which text a block
    /// flows: `Block::render` (marker-stripped, span-styled) or the block's raw
    /// source slice (one plain style). Geometry, wrapping, decorations and hit
    /// testing are shared, so a caret computed on one side of the switch lands
    /// on the other side without a special case.
    pub fn layout_mode(
        doc: &Document,
        theme: &EditorTheme,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
        mode: LayoutMode,
    ) -> Self {
        Self::layout_mode_marked(doc, theme, metrics, origin, width, mode, None)
    }

    /// [`DocLayout::layout_mode`], with an IME composition spliced into the caret
    /// block's source projection.
    ///
    /// This is where design D4 lands: the composition string is *not* in the
    /// rope, so it is inserted into the displayed source slice here, its own
    /// underlined fragment run, and the block's [`BlockBox::marked`] remembers
    /// where it sits. Everything downstream (caret geometry, `bounds_for_range`,
    /// hit testing) then works in that composed coordinates system, which is what
    /// makes the candidate window follow the composition instead of the buffer.
    pub fn layout_mode_marked(
        doc: &Document,
        theme: &EditorTheme,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
        mode: LayoutMode,
        marked: Option<&MarkedText>,
    ) -> Self {
        Self::layout_mode_marked_cached(doc, theme, metrics, origin, width, mode, marked, None)
    }

    /// [`DocLayout::layout_mode_marked`] with a block cache that survives between
    /// frames (task 7.1, 块级缓存). Headless callers get placeholder object metrics.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_mode_marked_cached(
        doc: &Document,
        theme: &EditorTheme,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
        mode: LayoutMode,
        marked: Option<&MarkedText>,
        cache: Option<&mut BlockCache>,
    ) -> Self {
        Self::layout_mode_marked_cached_media(
            doc, theme, metrics, origin, width, mode, marked, &NoMedia, 0, cache,
        )
    }

    /// The same layout with real image/math metrics.
    ///
    /// `media_generation` is incremented by the editor whenever an image finishes
    /// loading (or fails), so a cached block geometry is not reused across a size
    /// change. Math metrics are deterministic and do not need a generation.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_mode_marked_cached_media(
        doc: &Document,
        theme: &EditorTheme,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
        mode: LayoutMode,
        marked: Option<&MarkedText>,
        media: &dyn MediaMeasures,
        media_generation: u64,
        cache: Option<&mut BlockCache>,
    ) -> Self {
        Self::layout_windowed_media(
            doc,
            theme,
            metrics,
            origin,
            width,
            mode,
            marked,
            media,
            media_generation,
            cache,
            None,
        )
    }

    /// Whole-document layout, or its viewport-first half: blocks inside the
    /// window (the visible band plus margin) are measured now, everything else
    /// keeps an estimated height until the view or the background pass fills it
    /// in with exact geometry.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_windowed_media(
        doc: &Document,
        theme: &EditorTheme,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
        mode: LayoutMode,
        marked: Option<&MarkedText>,
        media: &dyn MediaMeasures,
        media_generation: u64,
        cache: Option<&mut BlockCache>,
        window: Option<Viewport>,
    ) -> Self {
        let started = std::time::Instant::now();
        let page_left = px(origin.x.as_f32() + theme.page_pad);
        let page_right =
            px((origin.x.as_f32() + width.as_f32() - theme.page_pad).max(page_left.as_f32() + 40.));
        let mut y = px(origin.y.as_f32() + theme.page_pad);
        let mut blocks = Vec::with_capacity(doc.blocks().len());
        let mut ordinals: Vec<usize> = Vec::new();
        let mut ordinal_log: Vec<Option<usize>> = Vec::with_capacity(doc.blocks().len());
        let mut measured_blocks = 0usize;
        let mut estimated_blocks = 0usize;
        let mut cache = cache;
        if let Some(cache) = cache.as_deref_mut() {
            cache.prepare(CacheStamp {
                revision: doc.revision(),
                len_bytes: doc.len_bytes(),
                width: width.as_f32().to_bits(),
                origin_x: origin.x.as_f32().to_bits(),
                font: metrics.tag(),
                theme: theme_tag(theme),
                media: media_generation,
            });
        }

        for (block_idx, block) in doc.blocks().iter().enumerate() {
            let kind = block.kind.clone();
            if block_idx > 0 && !matches!(kind, BlockKind::Gap) {
                y = px(y.as_f32() + gap_before(&kind, theme));
            }
            // The list-ordinal counter belongs to the *sequence*, not to one block,
            // so it advances on a cache hit too.
            let mut ordinal = None;
            match &kind {
                BlockKind::FencedCode { .. }
                | BlockKind::IndentedCode
                | BlockKind::BlockQuote
                | BlockKind::Rule
                | BlockKind::Html => ordinals.clear(),
                BlockKind::ListItem { ordered, level } => {
                    ordinal = next_ordinal(&mut ordinals, *level, *ordered);
                }
                _ => {}
            }
            let top = y;
            // Only the caret block's source is copied out of the buffer, and only
            // its own bytes -- optimisation ② of findings H1: this used to be one
            // whole-document `doc.text()` per frame whenever a caret existed.
            let mut mark_local: Option<Range<usize>> = None;
            let source_text: Option<String> = if mode.is_source(block_idx) {
                let slice = source_slice(doc, &block.src);
                match marked.filter(|m| !m.is_empty()) {
                    Some(m) => {
                        let (spliced, local) = m.splice(&slice, block.src.start);
                        mark_local = Some(local..local + m.len());
                        Some(spliced)
                    }
                    None => Some(slice),
                }
            } else {
                None
            };
            let source_mode = source_text.is_some();
            let source_view: Option<SourceView<'_>> =
                source_text.as_deref().map(|text| SourceView {
                    text,
                    marked: mark_local.clone(),
                });
            let key = BlockKey {
                kind: kind_tag(&kind),
                render: (block.render.as_ptr() as usize, block.render.len()),
                source: source_text.as_deref().map(|t| (hash_str(t), t.len())),
                marked: match (&source_text, &mark_local) {
                    (Some(text), Some(range)) => {
                        let inside = text.get(range.clone()).unwrap_or_default();
                        Some((range.start, range.end, hash_str(inside)))
                    }
                    _ => None,
                },
                ordinal,
                media: media_generation,
            };
            ordinal_log.push(ordinal);
            let mode_for_block = if source_mode {
                BlockMode::Source
            } else {
                BlockMode::Rendered
            };
            let cached = match cache.as_deref_mut() {
                Some(cache) => cache.get(block_idx, &key),
                None => None,
            };
            let estimated = estimate_height(theme, block);
            let in_window = match window {
                None => true,
                Some(window) => {
                    let top_f = top.as_f32();
                    top_f <= window.bottom && top_f + estimated.max(1.0) >= window.top
                }
            };
            let (boxed, height) = match cached {
                Some(geometry) => {
                    let height = geometry.height;
                    let boxed = if in_window {
                        geometry.materialize(
                            block,
                            block_idx,
                            top,
                            mode_for_block,
                            source_text,
                            mark_local,
                        )
                    } else {
                        placeholder_block(block, block_idx, top, height, theme, page_left, page_right)
                    };
                    (boxed, height)
                }
                None if in_window => {
                    let geometry = block_geometry(
                        theme,
                        metrics.as_ref(),
                        media,
                        block,
                        block_idx,
                        page_left,
                        page_right,
                        origin.x,
                        ordinal,
                        source_view.as_ref(),
                    );
                    let height = geometry.height;
                    if let Some(cache) = cache.as_deref_mut() {
                        cache.put(block_idx, key, geometry.clone());
                    }
                    (
                        geometry.materialize(
                            block,
                            block_idx,
                            top,
                            mode_for_block,
                            source_text,
                            mark_local,
                        ),
                        height,
                    )
                }
                None => {
                    let height = px(estimated);
                    (
                        placeholder_block(
                            block, block_idx, top, height, theme, page_left, page_right,
                        ),
                        height,
                    )
                }
            };
            if boxed.measured {
                measured_blocks += 1;
            } else {
                estimated_blocks += 1;
            }
            y = px(top.as_f32() + height.as_f32());
            blocks.push(boxed);
        }

        let height = px((y.as_f32() - origin.y.as_f32() + theme.page_pad).max(theme.page_pad));
        Self {
            origin,
            size: size(width, height),
            content_width: px(page_right.as_f32() - page_left.as_f32()),
            mode,
            blocks,
            marked: marked.cloned(),
            metrics_label: metrics.label(),
            metrics,
            theme: theme.clone(),
            measure_ms: started.elapsed().as_secs_f64() * 1000.,
            shape_ms: 0.,
            shaped_fragments: 0,
            culled_fragments: 0,
            measured_blocks,
            estimated_blocks,
            ordinals: ordinal_log,
            window,
        }
    }

    /// Measure every placeholder that intersects the viewport, then recompute the
    /// block tops. Returns how many slots were filled in.
    ///
    /// The loop repeats because measuring one block changes the top of every
    /// later block, which can bring another placeholder into the window. It is
    /// bounded: if the estimates were good the second pass measures nothing.
    pub fn ensure_measured(
        &mut self,
        doc: &Document,
        media: &dyn MediaMeasures,
        media_generation: u64,
        viewport: Viewport,
        mut cache: Option<&mut BlockCache>,
    ) -> usize {
        let mut measured = 0usize;
        for _pass in 0..8 {
            let mut changed = false;
            for block_idx in 0..self.blocks.len() {
                if self.blocks[block_idx].measured {
                    continue;
                }
                let (top, height) = {
                    let block = &self.blocks[block_idx];
                    (block.bounds.origin.y, block.bounds.size.height)
                };
                let top_f = top.as_f32();
                if !viewport.intersects(top_f, top_f + height.as_f32()) {
                    continue;
                }
                if self.measure_slot(doc, media, media_generation, block_idx, cache.as_deref_mut()) {
                    measured += 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
            self.reflow();
        }
        measured
    }

    /// Measure one placeholder slot in place. Returns false when the index is out
    /// of range or already measured.
    fn measure_slot(
        &mut self,
        doc: &Document,
        media: &dyn MediaMeasures,
        media_generation: u64,
        block_idx: usize,
        mut cache: Option<&mut BlockCache>,
    ) -> bool {
        if self.blocks.get(block_idx).is_none_or(|block| block.measured) {
            return false;
        }
        let Some(block) = doc.blocks().get(block_idx) else {
            return false;
        };
        let top = self.blocks[block_idx].bounds.origin.y;
        let page_left = px(self.origin.x.as_f32() + self.theme.page_pad);
        let page_right = px(
            (self.origin.x.as_f32() + self.size.width.as_f32() - self.theme.page_pad)
                .max(page_left.as_f32() + 40.),
        );
        let mut mark_local: Option<Range<usize>> = None;
        let source_text: Option<String> = if self.mode.is_source(block_idx) {
            let slice = source_slice(doc, &block.src);
            match self.marked.as_ref().filter(|m| !m.is_empty()) {
                Some(m) => {
                    let (spliced, local) = m.splice(&slice, block.src.start);
                    mark_local = Some(local..local + m.len());
                    Some(spliced)
                }
                None => Some(slice),
            }
        } else {
            None
        };
        let key = BlockKey {
            kind: kind_tag(&block.kind),
            render: (block.render.as_ptr() as usize, block.render.len()),
            source: source_text.as_deref().map(|t| (hash_str(t), t.len())),
            marked: match (&source_text, &mark_local) {
                (Some(text), Some(range)) => {
                    let inside = text.get(range.clone()).unwrap_or_default();
                    Some((range.start, range.end, hash_str(inside)))
                }
                _ => None,
            },
            ordinal: self.ordinals.get(block_idx).copied().flatten(),
            media: media_generation,
        };
        let cached = match cache.as_deref_mut() {
            Some(cache) => cache.get(block_idx, &key),
            None => None,
        };
        let geometry = match cached {
            Some(geometry) => geometry,
            None => {
                let source_view = source_text.as_deref().map(|text| SourceView {
                    text,
                    marked: mark_local.clone(),
                });
                let geometry = block_geometry(
                    &self.theme,
                    self.metrics.as_ref(),
                    media,
                    block,
                    block_idx,
                    page_left,
                    page_right,
                    self.origin.x,
                    self.ordinals.get(block_idx).copied().flatten(),
                    source_view.as_ref(),
                );
                if let Some(cache) = cache.as_deref_mut() {
                    cache.put(block_idx, key, geometry.clone());
                }
                geometry
            }
        };
        let mode = if source_text.is_some() {
            BlockMode::Source
        } else {
            BlockMode::Rendered
        };
        self.blocks[block_idx] =
            geometry.materialize(block, block_idx, top, mode, source_text, mark_local);
        self.measured_blocks += 1;
        self.estimated_blocks = self.estimated_blocks.saturating_sub(1);
        true
    }

    /// Measure an explicit block range (used when the caret or a click refers to a
    /// block the viewport window has not reached yet). Reflows once at the end.
    pub fn ensure_blocks(
        &mut self,
        doc: &Document,
        media: &dyn MediaMeasures,
        media_generation: u64,
        range: std::ops::RangeInclusive<usize>,
        mut cache: Option<&mut BlockCache>,
    ) -> usize {
        let last = (*range.end()).min(self.blocks.len().saturating_sub(1));
        let first = *range.start();
        let mut measured = 0usize;
        for block_idx in first..=last {
            if self.measure_slot(doc, media, media_generation, block_idx, cache.as_deref_mut()) {
                measured += 1;
            }
        }
        if measured > 0 {
            self.reflow();
        }
        measured
    }

    /// The list ordinal of one block, as the layout pass assigned it.
    pub fn ordinal(&self, block_idx: usize) -> Option<usize> {
        self.ordinals.get(block_idx).copied().flatten()
    }

    /// Unmeasured block indices ordered by distance from the viewport: the first
    /// unmeasured block below the band, then the first above, then outward.
    /// `limit` bounds one frame's background work.
    pub fn unmeasured_near(&self, viewport: Viewport, limit: usize) -> Vec<usize> {
        if limit == 0 || self.estimated_blocks == 0 {
            return Vec::new();
        }
        let start = self
            .blocks
            .partition_point(|block| block.bounds.origin.y.as_f32() <= viewport.bottom);
        let mut below = start;
        let mut above = start;
        let mut out = Vec::with_capacity(limit);
        while out.len() < limit && (below < self.blocks.len() || above > 0) {
            if below < self.blocks.len() {
                if !self.blocks[below].measured {
                    out.push(below);
                    if out.len() == limit {
                        break;
                    }
                }
                below += 1;
            }
            if above > 0 {
                above -= 1;
                if !self.blocks[above].measured {
                    out.push(above);
                    if out.len() == limit {
                        break;
                    }
                }
            }
            if below >= self.blocks.len() && above == 0 {
                break;
            }
        }
        out
    }

    /// Fill the block cache for blocks outside the measured window. The current
    /// layout is not mutated: the next frame re-derives its geometry from the
    /// cache, so background work is paid once and reused.
    ///
    /// Returns how many blocks were measured. Stops early once `budget_ms` has
    /// elapsed so a frame never overruns because of background work.
    pub fn measure_background(
        &mut self,
        doc: &Document,
        media: &dyn MediaMeasures,
        media_generation: u64,
        indices: &[usize],
        cache: &mut BlockCache,
        budget_ms: f64,
    ) -> usize {
        let started = std::time::Instant::now();
        let mut measured = 0usize;
        for block_idx in indices {
            if started.elapsed().as_secs_f64() * 1000.0 > budget_ms {
                break;
            }
            // Installing the geometry (not just caching it) is what lets the next
            // round move past this block and what makes the scrollbar converge.
            if self.measure_slot(doc, media, media_generation, *block_idx, Some(cache)) {
                measured += 1;
            }
        }
        if measured > 0 {
            self.reflow();
        }
        measured
    }


    /// Give the caret a line box when it sits after the final newline of the
    /// source-projected block.
    ///
    /// A paragraph owns its terminating newline, but the flow draws only its
    /// non-empty lines: "hello\n" is one block whose visible text is "hello".
    /// After Enter the caret is at the block end, past that newline, and belongs
    /// on a new empty line — every editor shows it. Without this the caret maps
    /// to a second line the block box does not contain, so the page does not grow
    /// and the first Enter looks like it did nothing (the second one only works
    /// because it creates a real gap block).
    ///
    /// Returns true when the layout grew by one or more line boxes.
    pub fn ensure_caret_line(
        &mut self,
        caret: Option<usize>,
        source_block: Option<usize>,
    ) -> bool {
        let (Some(caret), Some(block_idx)) = (caret, source_block) else {
            return false;
        };
        let Some(block) = self.blocks.get(block_idx) else {
            return false;
        };
        // Only the position after the block's own final newline needs the extra
        // line; every earlier position already owns a line.
        if caret != block.src.end || block.src.end <= block.src.start {
            return false;
        }
        let text = block.source_text.as_deref().unwrap_or(&block.render);
        if !text.ends_with('\n') {
            return false;
        }
        // A blank run (a Gap of newlines) already draws one box per newline.
        if text.trim().is_empty() {
            return false;
        }
        let line_height = block.line_height.as_f32();
        if line_height <= 0. {
            return false;
        }
        let logical_lines = text.matches('\n').count() + 1;
        let produced = (block.bounds.size.height.as_f32() / line_height).round() as usize;
        if produced >= logical_lines {
            return false;
        }
        let extra = px((logical_lines - produced) as f32 * line_height);
        self.blocks[block_idx].bounds.size.height = px(
            self.blocks[block_idx].bounds.size.height.as_f32() + extra.as_f32(),
        );
        for block in &mut self.blocks[block_idx + 1..] {
            block.bounds.origin.y = px(block.bounds.origin.y.as_f32() + extra.as_f32());
            for span in &mut block.spans {
                span.origin.y = px(span.origin.y.as_f32() + extra.as_f32());
            }
            for decor in &mut block.decors {
                match decor {
                    BlockDecor::Quad { bounds, .. } => {
                        bounds.origin.y = px(bounds.origin.y.as_f32() + extra.as_f32());
                    }
                    BlockDecor::Text { origin, .. } => {
                        *origin = point(origin.x, px(origin.y.as_f32() + extra.as_f32()));
                    }
                }
            }
            for object in &mut block.objects {
                object.bounds.origin.y = px(object.bounds.origin.y.as_f32() + extra.as_f32());
                object.baseline = px(object.baseline.as_f32() + extra.as_f32());
            }
        }
        self.size.height = px(self.size.height.as_f32() + extra.as_f32());
        true
    }

    /// Recompute every block top from the stored heights. O(blocks) additions;
    /// called only when a measurement changed a height.
    pub fn reflow(&mut self) {
        let theme = &self.theme;
        let mut y = px(self.origin.y.as_f32() + theme.page_pad);
        for (index, block) in self.blocks.iter_mut().enumerate() {
            if index > 0 && !matches!(block.kind, BlockKind::Gap) {
                y = px(y.as_f32() + gap_before(&block.kind, theme));
            }
            block.bounds.origin.y = y;
            y = px(y.as_f32() + block.bounds.size.height.as_f32());
        }
        self.size.height =
            px((y.as_f32() - self.origin.y.as_f32() + theme.page_pad).max(theme.page_pad));
    }
    /// Move the whole layout (used once, from local coordinates into the
    /// element's window bounds before publishing it for hit testing).
    pub fn translate(&mut self, by: Point<Pixels>) {
        self.origin = point(
            px(self.origin.x.as_f32() + by.x.as_f32()),
            px(self.origin.y.as_f32() + by.y.as_f32()),
        );
        for block in &mut self.blocks {
            block.bounds.origin.x = px(block.bounds.origin.x.as_f32() + by.x.as_f32());
            block.bounds.origin.y = px(block.bounds.origin.y.as_f32() + by.y.as_f32());
            for span in &mut block.spans {
                span.origin = point(
                    px(span.origin.x.as_f32() + by.x.as_f32()),
                    px(span.origin.y.as_f32() + by.y.as_f32()),
                );
            }
            for decor in &mut block.decors {
                match decor {
                    BlockDecor::Quad { bounds, .. } => {
                        bounds.origin.x = px(bounds.origin.x.as_f32() + by.x.as_f32());
                        bounds.origin.y = px(bounds.origin.y.as_f32() + by.y.as_f32());
                    }
                    BlockDecor::Text { origin, .. } => {
                        *origin = point(
                            px(origin.x.as_f32() + by.x.as_f32()),
                            px(origin.y.as_f32() + by.y.as_f32()),
                        );
                    }
                }
            }
        }
    }

    pub fn metrics(&self) -> &dyn Metrics {
        self.metrics.as_ref()
    }

    pub fn block_bounds(&self, block_idx: usize) -> Option<Bounds<Pixels>> {
        self.blocks.get(block_idx).map(|b| b.bounds)
    }

    /// Block whose vertical band contains `y`. Blocks are ordered by y, so the
    /// first band that contains the point wins; zero-height blocks (gaps) never
    /// swallow a hit.
    pub fn block_at_y(&self, y: Pixels) -> Option<usize> {
        for block in &self.blocks {
            if block.bounds.size.height.as_f32() <= 0. {
                continue;
            }
            if y < block.bounds.origin.y {
                break;
            }
            if y < block.bounds.bottom() {
                return Some(block.block_idx);
            }
        }
        // Between/after blocks: the last block that starts at or above `y`.
        self.blocks
            .iter()
            .rev()
            .find(|b| b.bounds.origin.y <= y)
            .map(|b| b.block_idx)
            .or_else(|| self.blocks.first().map(|b| b.block_idx))
    }

    /// Nearest block for a point, ignoring x entirely (for clicks in the gutter).
    pub fn block_at_point(&self, p: Point<Pixels>) -> Option<usize> {
        self.block_at_y(p.y)
    }

    /// Point -> an atomic image/formula, when the point is inside one. Checked
    /// before [`Self::hit`] because objects have no text position of their own.
    pub fn object_at(&self, p: Point<Pixels>) -> Option<ObjectHit> {
        let block_idx = self.block_at_y(p.y)?;
        let block = self.blocks.get(block_idx)?;
        if !block.measured {
            return None;
        }
        let object = block.objects.iter().find(|object| {
            let bounds = object.bounds;
            p.x >= bounds.left()
                && p.x < bounds.right()
                && p.y >= bounds.top()
                && p.y < bounds.bottom()
        })?;
        let center = object.bounds.left().as_f32() + object.bounds.size.width.as_f32() * 0.5;
        Some(ObjectHit {
            block_idx,
            span_idx: object.span_idx,
            source: object.source.clone(),
            right_half: p.x.as_f32() > center,
        })
    }

    /// Point -> `(block_idx, render_byte)`. The reverse of [`Self::caret_bounds`].
    pub fn hit(&self, p: Point<Pixels>) -> Option<RenderHit> {
        let block_idx = self.block_at_y(p.y)?;
        let block = self.blocks.get(block_idx)?;
        // A placeholder has no fragments to hit. Visible blocks are always
        // measured, so this only rejects clicks in regions no frame has shown.
        if !block.measured {
            return None;
        }
        let blank_gap = matches!(block.kind, BlockKind::Gap)
            && block.display_text().chars().all(|ch| ch == '\n');
        if block.spans.is_empty() || blank_gap {
            // A block without fragments — a run of blank lines, or a rule — has no
            // fragment to measure, but it still owns one line box per line. So the
            // click has to land on **the line it was made in**, not always on the
            // block's first byte: clicking the 5th of five blank lines used to jump
            // to the 1st.
            //
            // This is the third place the "a `Gap` has no spans" hole showed up.
            // `caret_x` and `line_tops` were fixed together with the
            // caret-follows-text work; `hit` is the other side of the same mapping,
            // and while it was missing the two disagreed about where a blank run's
            // lines are.
            let line = blank_line_at_y(block, p.y.as_f32());
            return Some(RenderHit {
                block_idx,
                render_byte: blank_line_start(block, line),
                span_idx: 0,
            });
        }

        // Candidates: fragments on the line under the pointer.
        let on_line: Vec<&DrawSpan> = block
            .spans
            .iter()
            .filter(|s| {
                let top = s.origin.y.as_f32();
                p.y.as_f32() >= top && p.y.as_f32() < top + s.line_height.as_f32()
            })
            .collect();
        let pool: Vec<&DrawSpan> = if on_line.is_empty() {
            block.spans.iter().collect()
        } else {
            on_line
        };

        let mut best: Option<(f32, &DrawSpan)> = None;
        for span in &pool {
            let left = span.origin.x.as_f32();
            let right = left + span.width.as_f32();
            let distance = if p.x.as_f32() < left {
                left - p.x.as_f32()
            } else if p.x.as_f32() > right {
                p.x.as_f32() - right
            } else {
                0.
            };
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, span));
            }
        }
        let span = best?.1;
        let local =
            px((p.x.as_f32() - span.origin.x.as_f32()).clamp(0., span.width.as_f32().max(0.)));
        let index = nearest_boundary(
            self.metrics.as_ref(),
            &span.text,
            span.font_size,
            span.style,
            local,
        );
        Some(RenderHit {
            block_idx,
            render_byte: span.render.start + index,
            span_idx: span.span_idx,
        })
    }

    /// Fragment owning a byte offset of a block's painted text, plus the x
    /// offset inside it. This is the primitive `bounds_for_range` (IME, group 6)
    /// and caret painting both need.
    pub fn caret_x(
        &self,
        block_idx: usize,
        render_byte: usize,
    ) -> Option<(Pixels, Pixels, Pixels)> {
        let block = self.blocks.get(block_idx)?;
        if let Some(object) = block
            .objects
            .iter()
            .find(|o| o.render.start <= render_byte && render_byte <= o.render.end)
        {
            let x = if render_byte == object.render.start {
                object.bounds.left()
            } else {
                object.bounds.right()
            };
            return Some((x, object.bounds.top(), object.bounds.size.height));
        }
        if block.spans.is_empty() {
            // A block without fragments — a blank line, or a rule — still has a
            // caret: it sits at the content edge of **its own** line box.
            let y = px(block.bounds.origin.y.as_f32()
                + blank_line_index(block, render_byte) as f32 * block.line_height.as_f32());
            return Some((block.content_left, y, block.line_height));
        }

        let lines = line_ranges(block);
        let line_idx = line_index_for_byte(block, render_byte);
        let line = lines.get(line_idx)?;
        let line_spans: Vec<&DrawSpan> = block
            .spans
            .iter()
            .filter(|span| span.render.start >= line.start && span.render.start <= line.end)
            .collect();
        let line_top = line_spans.first().map_or(
            px(block.bounds.origin.y.as_f32() + line_idx as f32 * block.line_height.as_f32()),
            |span| span.origin.y,
        );
        if line.start == line.end || line_spans.is_empty() {
            return Some((block.content_left, line_top, block.line_height));
        }
        let first = line_spans[0];
        if render_byte <= first.render.start {
            return Some((first.origin.x, first.origin.y, first.line_height));
        }
        let Some(span) = line_spans
            .iter()
            .rev()
            .find(|span| span.render.start <= render_byte)
        else {
            return Some((first.origin.x, first.origin.y, first.line_height));
        };
        let local = render_byte
            .saturating_sub(span.render.start)
            .min(span.render.end.saturating_sub(span.render.start))
            .min(span.text.len());
        let local = snap_floor(&span.text, local);
        let raw = span.origin.x
            + self
                .metrics
                .x_for_index(&span.text, span.font_size, span.style, local);
        let x = if block.mode == BlockMode::Rendered {
            raw.min(block.bounds.right() - px(2.0))
                .max(block.bounds.left())
        } else {
            raw
        };
        Some((x, span.origin.y, span.line_height))
    }

    /// Visual line ranges of a block's painted text, excluding each `\n`.
    pub fn line_ranges(&self, block_idx: usize) -> Vec<Range<usize>> {
        self.blocks
            .get(block_idx)
            .map(line_ranges)
            .unwrap_or_default()
    }

    /// Which visual line owns `render_byte`. A byte at a newline belongs to the
    /// line it terminates; a byte just after it belongs to the next line.
    pub fn line_index_for_byte(&self, block_idx: usize, render_byte: usize) -> Option<usize> {
        self.blocks
            .get(block_idx)
            .map(|block| line_index_for_byte(block, render_byte))
    }

    /// Geometry of one visual line: its box, its content edges and its render
    /// range (the newline itself is not part of the range).
    pub fn line_bounds(&self, block_idx: usize, line: usize) -> Option<LineBox> {
        let block = self.blocks.get(block_idx)?;
        let ranges = line_ranges(block);
        let range = ranges.get(line)?.clone();
        let spans: Vec<&DrawSpan> = block
            .spans
            .iter()
            .filter(|span| span.render.start >= range.start && span.render.start <= range.end)
            .collect();
        let (top, height, left, right) = match (spans.first(), spans.last()) {
            (Some(first), Some(last)) => (
                first.origin.y,
                first.line_height,
                first.origin.x,
                px(last.origin.x.as_f32() + last.width.as_f32()),
            ),
            _ => (
                px(block.bounds.origin.y.as_f32() + line as f32 * block.line_height.as_f32()),
                block.line_height,
                block.content_left,
                block.content_left,
            ),
        };
        Some(LineBox {
            top,
            bottom: px(top.as_f32() + height.as_f32()),
            left,
            right,
            height,
            render: range,
        })
    }

    /// Caret rectangle for `(block_idx, render_byte)`: 2px wide, one line tall.
    pub fn caret_bounds(&self, block_idx: usize, render_byte: usize) -> Option<Bounds<Pixels>> {
        let (x, y, line_height) = self.caret_x(block_idx, render_byte)?;
        Some(Bounds::new(point(x, y), size(px(2.), line_height)))
    }

    /// How this block was projected.
    pub fn block_mode(&self, block_idx: usize) -> Option<BlockMode> {
        self.blocks.get(block_idx).map(|b| b.mode)
    }

    /// Top of every visual line of a block, in document order.
    ///
    /// Arrow up/down walks this instead of guessing from `line_height`, so it
    /// also works for blocks whose height comes from a decoration (a blank line,
    /// a code plate) rather than from a fragment.
    pub fn line_tops(&self, block_idx: usize) -> Vec<Pixels> {
        let Some(block) = self.blocks.get(block_idx) else {
            return Vec::new();
        };
        let mut tops: Vec<Pixels> = Vec::new();
        for span in &block.spans {
            if tops
                .last()
                .is_none_or(|t| (t.as_f32() - span.origin.y.as_f32()).abs() > 0.5)
            {
                tops.push(span.origin.y);
            }
        }
        if tops.is_empty() && block.bounds.size.height.as_f32() > 0. {
            // A fragment-less block owns one line box per newline in its painted text:
            // a run of blank lines is walkable line by line instead of collapsing to a
            // single top (which is what made the caret stick while the run grew).
            let lines = blank_line_count(block);
            for line in 0..lines.max(1) {
                tops.push(px(
                    block.bounds.origin.y.as_f32() + line as f32 * block.line_height.as_f32()
                ));
            }
        }
        tops
    }

    /// `(line_index, render_byte)` for a `y` inside a fragment-less block.
    ///
    /// This is the mapping [`DocLayout::hit`] uses there, exposed so callers (the
    /// `--dump-cursor` probe) can assert against the shipped rule instead of
    /// re-deriving it. `None` for a block that does have fragments.
    pub fn blank_line_of(&self, block_idx: usize, y: Pixels) -> Option<(usize, usize)> {
        let block = self.blocks.get(block_idx)?;
        if !block.spans.is_empty() {
            return None;
        }
        let line = blank_line_at_y(block, y.as_f32());
        Some((line, blank_line_start(block, line)))
    }

    /// Index of the visual line that holds `render_byte`.
    pub fn line_index(&self, block_idx: usize, render_byte: usize) -> Option<usize> {
        let block = self.blocks.get(block_idx)?;
        if block.spans.is_empty() {
            // The same mapping `caret_x` and `hit` use, so the arrow keys walk a run
            // of blank lines line by line instead of collapsing it to line 0.
            return Some(blank_line_index(block, render_byte));
        }
        let y = block
            .spans
            .iter()
            .find(|s| s.render.start <= render_byte && render_byte <= s.render.end)
            .map_or(block.bounds.origin.y, |s| s.origin.y);
        let tops = self.line_tops(block_idx);
        Some(
            tops.iter()
                .position(|t| (t.as_f32() - y.as_f32()).abs() <= 0.5)
                .unwrap_or(0),
        )
    }

    // ------------------------------------------------- source <-> projection

    /// Source offset -> `(block_idx, byte offset inside that block's painted
    /// text)`. The caret's only coordinate conversion for painting.
    pub fn display_byte_for_src(&self, doc: &Document, src: usize) -> Option<(usize, usize)> {
        let block_idx = doc.block_at_src(src)?;
        let block = self.blocks.get(block_idx)?;
        match block.mode {
            // Source mode *is* the identity projection of the block's bytes, with
            // the IME composition (which has none) spliced in at its anchor.
            BlockMode::Source => {
                let local = src.saturating_sub(block.src.start);
                let display = match &block.marked {
                    Some(m) if local > m.start => local + (m.end - m.start),
                    _ => local,
                };
                Some((block_idx, display.min(block.display_text().len())))
            }
            BlockMode::Rendered => doc.src_to_render_offset(src),
        }
    }

    /// Byte offset inside a block's painted text -> source offset.
    pub fn src_for_display_byte(
        &self,
        doc: &Document,
        block_idx: usize,
        display_byte: usize,
    ) -> Option<usize> {
        let block = self.blocks.get(block_idx)?;
        match block.mode {
            BlockMode::Source => {
                let display = display_byte.min(block.display_text().len());
                // An offset inside the composition has no buffer position of its
                // own: it belongs to the anchor, which is where the caret is.
                let local = match &block.marked {
                    Some(m) if display > m.start => {
                        display.saturating_sub(m.end - m.start).max(m.start)
                    }
                    _ => display,
                };
                Some(block.src.start + local)
            }
            BlockMode::Rendered => doc.render_offset_to_src(block_idx, display_byte),
        }
    }

    /// Composite byte offset -> `(block_idx, display_byte)`.
    ///
    /// "Composite" is the buffer with a live IME composition spliced in: the
    /// coordinate system `EntityInputHandler` speaks while composing (design D4)
    /// and the one the composition is painted in. With no composition it is
    /// exactly [`DocLayout::display_byte_for_src`].
    pub fn display_byte_for_composite(
        &self,
        doc: &Document,
        composite: usize,
    ) -> Option<(usize, usize)> {
        let Some(m) = self.marked.as_ref().filter(|m| !m.is_empty()) else {
            return self.display_byte_for_src(doc, composite);
        };
        if composite < m.anchor {
            return self.display_byte_for_src(doc, composite);
        }
        // From the anchor on, everything is painted inside the block that holds
        // it: the composition first, then the rest of that block's source.
        let block_idx = doc.block_at_src(m.anchor)?;
        let block = self.blocks.get(block_idx)?;
        let local = m.anchor.saturating_sub(block.src.start);
        let display = local + (composite - m.anchor);
        if block.marked.is_none() || display > block.display_text().len() {
            return self.display_byte_for_src(doc, m.to_source(composite));
        }
        Some((block_idx, display))
    }

    /// Caret rectangle for a composite byte offset: the candidate-window anchor.
    pub fn caret_bounds_composite(
        &self,
        doc: &Document,
        composite: usize,
    ) -> Option<Bounds<Pixels>> {
        let (block_idx, display_byte) = self.display_byte_for_composite(doc, composite)?;
        self.caret_bounds(block_idx, display_byte)
    }

    // ------------------------------------------------------------ reporting

    /// Human-readable dump: every block, its source range, its drawn fragments
    /// and their screen coordinates.
    pub fn report(&self) -> String {
        /// A 100k-character document has tens of thousands of blocks; the dump is
        /// for reading, so cap it (the counts above stay exact).
        const MAX_BLOCKS: usize = 200;
        let mut out = String::new();
        out.push_str(&format!(
            "measure: {}\npage: origin=({:.1},{:.1}) size=({:.1},{:.1}) content_width={:.1}\nblocks: {}\ndrawn fragments: {}\ntiming: layout {:.3} ms, shaping {:.3} ms\nculled: {} of {} fragments shaped for painting\n",
            self.metrics_label,
            self.origin.x.as_f32(),
            self.origin.y.as_f32(),
            self.size.width.as_f32(),
            self.size.height.as_f32(),
            self.content_width.as_f32(),
            self.blocks.len(),
            self.blocks.iter().map(|b| b.spans.len()).sum::<usize>(),
            self.measure_ms,
            self.shape_ms,
            self.culled_fragments,
            self.shaped_fragments + self.culled_fragments,
        ));
        if self.blocks.len() > MAX_BLOCKS {
            out.push_str(&format!(
                "(printing the first {MAX_BLOCKS} of {} blocks)\n",
                self.blocks.len()
            ));
        }
        if let Some(m) = &self.marked {
            out.push_str(&format!(
                "ime: composition {:?} at src {} ({} bytes) -> painted in block {:?}, display bytes {:?}\n",
                m.text,
                m.anchor,
                m.len(),
                self.blocks
                    .iter()
                    .find(|b| b.marked.is_some())
                    .map(|b| b.block_idx),
                self.blocks.iter().find_map(|b| b.marked.clone()),
            ));
        }
        for block in self.blocks.iter().take(MAX_BLOCKS) {
            out.push_str(&format!(
                "[{:>3}] {:<34} {:<9} src={:?} bounds=({:.1},{:.1},{:.1},{:.1}) fragments={}\n",
                block.block_idx,
                format!("{:?}", block.kind),
                match block.mode {
                    BlockMode::Rendered => "rendered",
                    BlockMode::Source => "SOURCE",
                },
                block.src,
                block.bounds.origin.x.as_f32(),
                block.bounds.origin.y.as_f32(),
                block.bounds.size.width.as_f32(),
                block.bounds.size.height.as_f32(),
                block.spans.len(),
            ));
            for decor in &block.decors {
                match decor {
                    BlockDecor::Quad { bounds, .. } => out.push_str(&format!(
                        "        decon quad  ({:.1},{:.1},{:.1},{:.1})\n",
                        bounds.origin.x.as_f32(),
                        bounds.origin.y.as_f32(),
                        bounds.size.width.as_f32(),
                        bounds.size.height.as_f32()
                    )),
                    BlockDecor::Text { text, origin, .. } => out.push_str(&format!(
                        "        decor text  {text:?} @ ({:.1},{:.1})\n",
                        origin.x.as_f32(),
                        origin.y.as_f32()
                    )),
                }
            }
            for span in &block.spans {
                out.push_str(&format!(
                    "        span {:>2} {:<22} @({:.1},{:.1}) w={:.1} render={:?} text={:?}\n",
                    span.span_idx,
                    style_label(span.style),
                    span.origin.x.as_f32(),
                    span.origin.y.as_f32(),
                    span.width.as_f32(),
                    span.render,
                    span.text,
                ));
            }
            for object in &block.objects {
                let label = match &object.kind {
                    ObjectKind::Image { .. } => "image",
                    ObjectKind::Math { display: true, .. } => "display-math",
                    ObjectKind::Math { display: false, .. } => "inline-math",
                    ObjectKind::Diagram { .. } => "diagram",
                    ObjectKind::Chart { .. } => "chart",
                };
                out.push_str(&format!(
                    "        object {:>2} {:<12} @({:.1},{:.1}) {}x{} src={:?} display={}\n",
                    object.span_idx,
                    label,
                    object.bounds.origin.x.as_f32(),
                    object.bounds.origin.y.as_f32(),
                    object.bounds.size.width.as_f32(),
                    object.bounds.size.height.as_f32(),
                    object.source,
                    object.display,
                ));
            }
        }
        out
    }

    /// Programmatic stand-in for the visual check.
    ///
    /// * `A1` painting is lossless: the fragments of a block concatenate to the
    ///   block's painted text ([`BlockBox::display_text`]) with newlines removed.
    /// * `A2` every painted fragment came from a visible span and slices
    ///   `Block::render` byte for byte •a `Marker` can never be painted. In the
    ///   source projection the painted text is the buffer slice instead.
    /// * `A3` on blocks that carry no `Degraded` span, the painted text contains
    ///   no Markdown marking characters (`**`, backtick, leading `#`, `- `).
    /// * `A5` the projection switch is block-local: at most one block is source,
    ///   it is the requested one, and it paints its raw Markdown source.
    pub fn checks(&self, doc: &Document) -> Vec<(String, bool, String)> {
        let mut results = Vec::new();

        let mut a1_bad = Vec::new();
        for block in &self.blocks {
            // A placeholder is outside the measured window: it paints nothing, so
            // it has no painted-vs-displayed contract to check.
            if !block.measured {
                continue;
            }
            if doc.blocks().get(block.block_idx).is_none() {
                a1_bad.push(format!("block {}: missing in document", block.block_idx));
                continue;
            }
            let painted = painted_text(block);
            let expected: String = block
                .display_text()
                .chars()
                .filter(|c| *c != '\n')
                .collect();
            if painted != expected {
                a1_bad.push(format!(
                    "block {}: painted {:?} != displayed {:?}",
                    block.block_idx, painted, expected
                ));
            }
        }
        results.push((
            "A1 painted text == the block's displayed text (newlines stripped)".to_string(),
            a1_bad.is_empty(),
            if a1_bad.is_empty() {
                format!(
                    "{} measured, {} estimated",
                    self.measured_blocks, self.estimated_blocks
                )
            } else {
                a1_bad.join("; ")
            },
        ));

        let mut a2_bad = Vec::new();
        let mut painted_fragments = 0usize;
        let buffer_text = if self.mode.source_block.is_some() {
            doc.text()
        } else {
            String::new()
        };
        for block in &self.blocks {
            if !block.measured {
                continue;
            }
            let Some(src) = doc.blocks().get(block.block_idx) else {
                continue;
            };
            match block.mode {
                BlockMode::Source => {
                    // The source projection is the buffer slice, byte for byte --
                    // plus a live IME composition, which is spliced in and is *not*
                    // in the buffer (design D4). Recomposed here so A2 keeps its
                    // exactness claim even mid-composition.
                    let slice = buffer_text.get(block.src.clone()).unwrap_or_default();
                    let expected: std::borrow::Cow<'_, str> =
                        match (&block.marked, self.marked.as_ref()) {
                            (Some(local), Some(m)) => {
                                let mut spliced = String::with_capacity(slice.len() + m.len());
                                spliced.push_str(&slice[..local.start]);
                                spliced.push_str(&m.text);
                                spliced.push_str(&slice[local.end..]);
                                std::borrow::Cow::Owned(spliced)
                            }
                            _ => std::borrow::Cow::Borrowed(slice),
                        };
                    if block.source_text.as_deref() != Some(expected.as_ref()) {
                        a2_bad.push(format!(
                            "block {}: source projection is not the block's buffer slice",
                            block.block_idx
                        ));
                    }
                    for span in &block.spans {
                        painted_fragments += 1;
                        if span.span_idx != SOURCE_SPAN {
                            a2_bad.push(format!(
                                "block {}: a source fragment claims span {}",
                                block.block_idx, span.span_idx
                            ));
                        }
                        if span.text.contains('\n') {
                            a2_bad.push(format!(
                                "block {}: a source fragment contains a newline",
                                block.block_idx
                            ));
                        }
                    }
                    continue;
                }
                BlockMode::Rendered => {}
            }
            if block.display_text() != src.render {
                a2_bad.push(format!(
                    "block {}: rendered projection is not block.render",
                    block.block_idx
                ));
            }
            for span in &block.spans {
                painted_fragments += 1;
                let Some(src_span) = src.spans.get(span.span_idx) else {
                    a2_bad.push(format!(
                        "block {}: span {} missing",
                        block.block_idx, span.span_idx
                    ));
                    continue;
                };
                if matches!(src_span.kind, SpanKind::Marker(_)) {
                    a2_bad.push(format!(
                        "block {} span {}: a zero-width Marker was painted",
                        block.block_idx, span.span_idx
                    ));
                }
                if !src_span.is_visible() {
                    a2_bad.push(format!(
                        "block {} span {}: invisible span was painted",
                        block.block_idx, span.span_idx
                    ));
                }
                if src.render.get(span.render.clone()) != Some(span.text.as_str()) {
                    a2_bad.push(format!(
                        "block {} span {}: fragment text does not slice render {:?}",
                        block.block_idx, span.span_idx, span.render
                    ));
                }
                if span.text.contains('\n') {
                    a2_bad.push(format!(
                        "block {} span {}: fragment contains a newline",
                        block.block_idx, span.span_idx
                    ));
                }
            }
        }
        results.push((
            "A2 no Marker/invisible span painted, fragments slice render exactly".to_string(),
            a2_bad.is_empty(),
            if a2_bad.is_empty() {
                format!("{painted_fragments} fragments")
            } else {
                a2_bad.join("; ")
            },
        ));

        // A3 checks the two ways a zero-width marker could still reach the
        // screen: (a) the model claims it is zero-width but the render range is
        // not empty, and (b) it consumed horizontal space, i.e. the first painted
        // fragment of a heading does not sit at the content edge. A heading also
        // must not paint a `#` at all.
        let mut a3_bad = Vec::new();
        let mut a3_blocks = 0usize;
        for block in &self.blocks {
            let Some(src) = doc.blocks().get(block.block_idx) else {
                continue;
            };
            for (i, span) in src.spans.iter().enumerate() {
                if matches!(span.kind, SpanKind::Marker(_)) && span.render.start != span.render.end
                {
                    a3_bad.push(format!(
                        "block {} span {i}: a Marker span has a non-empty render range",
                        block.block_idx
                    ));
                }
            }
            if !matches!(block.kind, BlockKind::Heading { .. }) {
                continue;
            }
            if block.mode == BlockMode::Source {
                // The caret block is *supposed* to show `## `; A5 checks that.
                continue;
            }
            a3_blocks += 1;
            let painted: String = block.spans.iter().map(|s| s.text.as_str()).collect();
            if painted.contains('#') {
                a3_bad.push(format!(
                    "block {}: heading painted text contains '#': {painted:?}",
                    block.block_idx
                ));
            }
            if let Some(first) = block.spans.first()
                && (first.origin.x.as_f32() - block.bounds.origin.x.as_f32()).abs() > 0.5
            {
                a3_bad.push(format!(
                    "block {}: first fragment at x={:.1} is shifted from the content edge {:.1}",
                    block.block_idx,
                    first.origin.x.as_f32(),
                    block.bounds.origin.x.as_f32()
                ));
            }
        }
        results.push((
            "A3 marker runs stay zero-width: no '#' in headings, no horizontal shift".to_string(),
            a3_bad.is_empty(),
            if a3_bad.is_empty() {
                format!("{a3_blocks} headings, all blocks' Marker spans zero-width")
            } else {
                a3_bad.join("; ")
            },
        ));

        // A4 is the group-4 contract: byte -> x -> byte must be stable, and a
        // block must be found again from a point inside its own box.
        let mut a4_bad = Vec::new();
        let mut a4_probes = 0usize;
        for block in &self.blocks {
            if block.bounds.size.height.as_f32() > 0. {
                let mid =
                    px(block.bounds.origin.y.as_f32() + block.bounds.size.height.as_f32() / 2.);
                match self.block_at_y(mid) {
                    Some(found) if found == block.block_idx => {}
                    other => a4_bad.push(format!(
                        "block {}: block_at_y returned {other:?}",
                        block.block_idx
                    )),
                }
            }
            for span in &block.spans {
                if a4_probes >= A4_MAX_PROBES {
                    break;
                }
                let mut local = 0usize;
                while local <= span.text.len() {
                    if a4_probes >= A4_MAX_PROBES {
                        break;
                    }
                    let render_byte = span.render.start + local;
                    a4_probes += 1;
                    if let Some((x, y, line_height)) = self.caret_x(block.block_idx, render_byte) {
                        let probe = point(
                            px(x.as_f32() + 0.5),
                            px(y.as_f32() + line_height.as_f32() / 2.),
                        );
                        match self.hit(probe) {
                            Some(hit) => match self.caret_x(hit.block_idx, hit.render_byte) {
                                Some((x2, _, _)) if (x2.as_f32() - x.as_f32()).abs() <= 1.5 => {}
                                other => a4_bad.push(format!(
                                    "block {} byte {render_byte}: x {:.1} -> hit byte {} -> x {other:?}",
                                    block.block_idx, x.as_f32(), hit.render_byte
                                )),
                            },
                            None => a4_bad.push(format!(
                                "block {} byte {render_byte}: hit at ({:.1},{:.1}) returned None",
                                block.block_idx, probe.x.as_f32(), probe.y.as_f32()
                            )),
                        }
                    }
                    let Some(ch) = span.text[local..].chars().next() else {
                        break;
                    };
                    local += ch.len_utf8();
                }
            }
        }
        results.push((
            "A4 caret x <-> hit round-trip and block_at_y self-consistency".to_string(),
            a4_bad.is_empty(),
            if a4_bad.is_empty() {
                format!(
                    "{a4_probes} probes{}",
                    if a4_probes >= A4_MAX_PROBES {
                        " (capped)"
                    } else {
                        ""
                    }
                )
            } else {
                a4_bad.join("; ")
            },
        ));

        // A5 is the "光标行源码还原" contract as a check on the layout alone: at
        // most one block is in the source projection, it is the requested one,
        // it paints the buffer slice verbatim, and the siblings stay rendered.
        let mut a5_bad = Vec::new();
        let mut source_blocks = 0usize;
        let mut source_painted = String::new();
        for block in &self.blocks {
            let requested = self.mode.source_block == Some(block.block_idx);
            match (block.mode, requested) {
                (BlockMode::Source, false) => a5_bad.push(format!(
                    "block {} is projected as source but is not the caret block",
                    block.block_idx
                )),
                (BlockMode::Rendered, true) => a5_bad.push(format!(
                    "block {} is the caret block but is still rendered",
                    block.block_idx
                )),
                (BlockMode::Source, true) => {
                    source_blocks += 1;
                    source_painted = block.spans.iter().map(|s| s.text.as_str()).collect();
                    let expected: String = block
                        .display_text()
                        .chars()
                        .filter(|c| *c != '\n')
                        .collect();
                    if source_painted != expected {
                        a5_bad.push(format!(
                            "block {}: source projection painted {source_painted:?} != {expected:?}",
                            block.block_idx
                        ));
                    }
                    if let BlockKind::Heading { level } = block.kind {
                        let marker = "#".repeat(level as usize) + " ";
                        if !source_painted.starts_with(&marker) {
                            a5_bad.push(format!(
                                "block {}: heading caret block does not start with {marker:?}: {source_painted:?}",
                                block.block_idx
                            ));
                        }
                    }
                }
                (BlockMode::Rendered, false) => {}
            }
        }
        if self.mode.source_block.is_some() && source_blocks != 1 {
            a5_bad.push(format!(
                "{source_blocks} blocks in the source projection, expected exactly 1"
            ));
        }
        results.push((
            "A5 exactly the caret block is projected as Markdown source".to_string(),
            a5_bad.is_empty(),
            if a5_bad.is_empty() {
                match self.mode.source_block {
                    Some(idx) => format!(
                        "caret block {idx} painted {source_painted:?}, {} blocks still rendered",
                        self.blocks.len() - source_blocks
                    ),
                    None => format!("no caret block; all {} blocks rendered", self.blocks.len()),
                }
            } else {
                a5_bad.join("; ")
            },
        ));

        results
    }
}

fn style_label(style: doc::Style) -> String {
    let mut parts = Vec::new();
    if style.bold {
        parts.push("bold");
    }
    if style.italic {
        parts.push("italic");
    }
    if style.code {
        parts.push("code");
    }
    if style.link {
        parts.push("link");
    }
    if parts.is_empty() {
        "plain".to_string()
    } else {
        parts.join("+")
    }
}

/// How many line boxes a fragment-less block owns: one per `\n` in its painted
/// text, and never zero.
///
/// This is the same count `block_geometry` turns into the block's height
/// (`BlockKind::Gap`), so the line boxes, the caret and the height always agree.
fn blank_line_count(block: &BlockBox) -> usize {
    block
        .display_text()
        .chars()
        .filter(|c| *c == '\n')
        .count()
        .max(1)
}

/// Which line box of a fragment-less block a byte offset of its painted text
/// belongs to.
///
/// `render_byte` counts bytes, and in a blank run every byte is a newline, so the
/// number of newlines before it *is* the line. The trailing newline terminates the
/// last line instead of opening a new one, hence the clamp.
fn blank_line_index(block: &BlockBox, render_byte: usize) -> usize {
    let text = block.display_text();
    let upto = snap_floor(text, render_byte.min(text.len()));
    let before = text[..upto].chars().filter(|c| *c == '\n').count();
    before.min(blank_line_count(block) - 1)
}

/// The inverse of [`blank_line_index`]: the byte offset at which line `line` of a
/// fragment-less block starts.
///
/// Line `k` starts right after the `k`-th newline of the run (line 0 starts at the
/// block's first byte), which is exactly the byte [`blank_line_index`] maps back to
/// `k` — so `hit` and `caret_x` are inverses inside a blank run too.
fn line_ranges(block: &BlockBox) -> Vec<Range<usize>> {
    let text = block.display_text();
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push(start..index);
            start = index + 1;
        }
    }
    ranges.push(start..text.len());
    ranges
}

fn line_index_for_byte(block: &BlockBox, render_byte: usize) -> usize {
    let text = block.display_text();
    let byte = render_byte.min(text.len());
    let mut line = 0usize;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            if byte <= index {
                return line;
            }
            line += 1;
        }
    }
    line
}

fn blank_line_start(block: &BlockBox, line: usize) -> usize {
    let text = block.display_text();
    if line == 0 {
        return 0;
    }
    let mut seen = 0usize;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            seen += 1;
            if seen == line {
                return index + ch.len_utf8();
            }
        }
    }
    text.len()
}

/// Which line box of a fragment-less block a `y` coordinate falls in, clamped to the
/// block's own lines.
///
/// The clamp is the block-edge rule the rest of `hit` follows: a point on the block's
/// top boundary is line 0, a point at or past its bottom boundary is the last line,
/// and nothing outside the block is ever reported.
fn blank_line_at_y(block: &BlockBox, y: f32) -> usize {
    let lines = blank_line_count(block);
    let line_height = block.line_height.as_f32();
    if !(line_height > 0.) || !y.is_finite() {
        return 0;
    }
    let offset = (y - block.bounds.origin.y.as_f32()) / line_height;
    (offset.floor().max(0.) as usize).min(lines - 1)
}

fn snap_floor(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

/// Previous char boundary strictly before `index`.
fn prev_boundary(text: &str, index: usize) -> usize {
    let mut i = snap_floor(text, index).saturating_sub(1);
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Nearest caret boundary to `x`.
///
/// `Metrics::index_for_x` rounds *up* (first boundary at or after `x`), which is
/// the wrong side for a click 0.5px into a wide CJK glyph: the caret must land on
/// the glyph start. So compare against the boundary before it and take the
/// closer one.
fn nearest_boundary(
    metrics: &dyn Metrics,
    text: &str,
    font_size: Pixels,
    style: doc::Style,
    x: Pixels,
) -> usize {
    let upper = snap_floor(
        text,
        metrics
            .index_for_x(text, font_size, style, x)
            .min(text.len()),
    );
    if upper == 0 {
        return 0;
    }
    let lower = prev_boundary(text, upper);
    let upper_x = metrics.x_for_index(text, font_size, style, upper).as_f32();
    let lower_x = metrics.x_for_index(text, font_size, style, lower).as_f32();
    if (x.as_f32() - lower_x).abs() <= (upper_x - x.as_f32()).abs() {
        lower
    } else {
        upper
    }
}

/// A height guessed from the block kind. Used only for blocks outside the
/// measured window, so a wrong guess costs scrollbar accuracy, never content:
/// the block is measured before it is painted. A Gap's guess is exact because
/// its line count is in its render text.
fn estimate_height(theme: &EditorTheme, block: &Block) -> f32 {
    let body = theme.body_size * theme.line_ratio;
    match &block.kind {
        BlockKind::Gap => {
            let lines = block.render.chars().filter(|ch| *ch == '\n').count().max(1);
            lines as f32 * body
        }
        BlockKind::Heading { level } => {
            let index = (*level as usize).saturating_sub(1).min(theme.heading_scale.len() - 1);
            body * theme.heading_scale[index].max(1.0) * theme.heading_line_ratio
                + theme.heading_gap
        }
        BlockKind::FencedCode { .. } | BlockKind::IndentedCode => body * 3.0,
        BlockKind::Table => body * 2.0,
        BlockKind::Rule => 24.0,
        _ => body,
    }
}

/// A slot with an estimated height and no paint commands. Its render text is
/// deliberately empty: copying every block's text is the whole-document cost the
/// windowed layout exists to avoid, and the clipboard/selection read the rope.
#[allow(clippy::too_many_arguments)]
fn placeholder_block(
    block: &Block,
    block_idx: usize,
    top: Pixels,
    height: Pixels,
    theme: &EditorTheme,
    page_left: Pixels,
    page_right: Pixels,
) -> BlockBox {
    BlockBox {
        block_idx,
        kind: block.kind.clone(),
        src: block.src.clone(),
        bounds: Bounds::new(
            point(page_left, top),
            size(px((page_right.as_f32() - page_left.as_f32()).max(0.)), height),
        ),
        render: String::new(),
        mode: BlockMode::Rendered,
        source_text: None,
        marked: None,
        content_left: page_left,
        line_height: px(theme.body_size * theme.line_ratio),
        spans: Vec::new(),
        decors: Vec::new(),
        objects: Vec::new(),
        measured: false,
    }
}

fn gap_before(kind: &BlockKind, theme: &EditorTheme) -> f32 {
    match kind {
        BlockKind::Gap => 0.,
        BlockKind::Heading { .. } => theme.heading_gap,
        BlockKind::FencedCode { .. }
        | BlockKind::IndentedCode
        | BlockKind::BlockQuote
        | BlockKind::Rule => theme.block_gap * 1.5,
        _ => theme.block_gap,
    }
}

fn next_ordinal(counts: &mut Vec<usize>, level: usize, ordered: bool) -> Option<usize> {
    if counts.len() <= level {
        counts.resize(level + 1, 0);
    }
    counts.truncate(level + 1);
    if ordered {
        counts[level] += 1;
        Some(counts[level])
    } else {
        counts[level] = 0;
        None
    }
}

fn bullet(
    theme: &EditorTheme,
    metrics: &dyn Metrics,
    indent: Pixels,
    top: Pixels,
    ordered: bool,
    ordinal: Option<usize>,
) -> BlockDecor {
    let text = if ordered {
        format!("{}.", ordinal.unwrap_or(1))
    } else {
        "\u{2022}".to_string()
    };
    let size = px(theme.body_size);
    let width = metrics.advance(&text, size, doc::Style::PLAIN);
    let x = px(
        (indent.as_f32() + theme.bullet_col - theme.block_gap - width.as_f32())
            .max(indent.as_f32()),
    );
    BlockDecor::Text {
        text,
        origin: point(x, top),
        font_size: size,
        line_height: px(theme.body_size * theme.line_ratio),
        color: theme.muted,
    }
}

// ---------------------------------------------------------------- block geometry

impl BlockGeometry {
    /// Turn a cached (top-relative) geometry into the block box for `top`.
    ///
    /// Everything below a block's top is a pure translation of the same geometry,
    /// which is what makes the cache sound: the arms of [`block_geometry`] only
    /// ever add `top` to a y coordinate.
    fn materialize(
        &self,
        block: &Block,
        block_idx: usize,
        top: Pixels,
        mode: BlockMode,
        source_text: Option<String>,
        marked: Option<Range<usize>>,
    ) -> BlockBox {
        let dy = top.as_f32();
        let mut spans = self.spans.clone();
        for span in &mut spans {
            span.origin.y = px(span.origin.y.as_f32() + dy);
        }
        let mut decors = self.decors.clone();
        for decor in &mut decors {
            match decor {
                BlockDecor::Quad { bounds, .. } => {
                    bounds.origin.y = px(bounds.origin.y.as_f32() + dy);
                }
                BlockDecor::Text { origin, .. } => {
                    origin.y = px(origin.y.as_f32() + dy);
                }
            }
        }
        let mut objects = self.objects.clone();
        for object in &mut objects {
            object.bounds.origin.y = px(object.bounds.origin.y.as_f32() + dy);
            object.baseline = px(object.baseline.as_f32() + dy);
        }
        BlockBox {
            block_idx,
            kind: block.kind.clone(),
            src: block.src.clone(),
            bounds: Bounds::new(
                point(self.box_left, top),
                size(
                    px(self.box_right.as_f32() - self.box_left.as_f32()),
                    px(self.height.as_f32().max(0.)),
                ),
            ),
            render: block.render.clone(),
            mode,
            source_text,
            marked,
            content_left: self.content_left,
            line_height: self.line_height,
            spans,
            decors,
            objects,
            measured: true,
        }
    }
}

/// Lay one block out with its top at `y = 0`, so the result can be re-used at any
/// top (see [`BlockGeometry::materialize`]).
///
/// This is the body that used to sit inline in
/// [`DocLayout::layout_mode_marked`]; splitting it out is what makes the block
/// cache possible without duplicating the six block shapes.
#[allow(clippy::too_many_arguments)]
fn block_geometry(
    theme: &EditorTheme,
    metrics: &dyn Metrics,
    media: &dyn MediaMeasures,
    block: &Block,
    block_idx: usize,
    page_left: Pixels,
    page_right: Pixels,
    origin_x: Pixels,
    ordinal: Option<usize>,
    source_view: Option<&SourceView<'_>>,
) -> BlockGeometry {
    let top = px(0.);
    let mut spans = Vec::new();
    let mut objects = Vec::new();
    let mut decors = Vec::new();
    let mut box_left = page_left;
    let mut box_right = page_right;

    let bottom = match &block.kind {
        // A `Gap` owns the bytes no Markdown block claimed: blank lines
        // and stray text. A blank gap keeps one line box per blank line,
        // because the caret has to be able to sit on an empty line.
        BlockKind::Gap => {
            if block.render.trim().is_empty() {
                let lines = block.render.chars().filter(|c| *c == '\n').count().max(1);
                px(top.as_f32() + lines as f32 * theme.body_size * theme.line_ratio)
            } else {
                Flow {
                    metrics,
                    media,
                    theme,
                    block,
                    block_idx,
                    font_size: px(theme.body_size),
                    line_height: px(theme.body_size * theme.line_ratio),
                    left: page_left,
                    right: page_right,
                    top,
                    color: theme.muted,
                    force_bold: false,
                    force_mono: false,
                    force_color: true,
                }
                .run_mode(source_view, &mut spans, &mut objects)
            }
        }
        BlockKind::Heading { level } => {
            let scale = theme.heading_scale[(*level as usize).clamp(1, 6) - 1];
            let heading_size = px(theme.body_size * scale);
            Flow {
                metrics,
                media,
                theme,
                block,
                block_idx,
                font_size: heading_size,
                line_height: px(heading_size.as_f32() * theme.heading_line_ratio),
                left: page_left,
                right: page_right,
                top,
                color: theme.text,
                force_bold: true,
                force_mono: false,
                force_color: true,
            }
            .run_mode(source_view, &mut spans, &mut objects)
        }
        BlockKind::Paragraph => Flow {
            metrics,
            media,
            theme,
            block,
            block_idx,
            font_size: px(theme.body_size),
            line_height: px(theme.body_size * theme.line_ratio),
            left: page_left,
            right: page_right,
            top,
            color: theme.text,
            force_bold: false,
            force_mono: false,
            force_color: false,
        }
        .run_mode(source_view, &mut spans, &mut objects),
        BlockKind::ListItem { ordered, level } => {
            let indent = px(page_left.as_f32() + theme.indent_per_level * *level as f32);
            let content_left = px(indent.as_f32() + theme.bullet_col);
            decors.push(bullet(theme, metrics, indent, top, *ordered, ordinal));
            Flow {
                metrics,
                media,
                theme,
                block,
                block_idx,
                font_size: px(theme.body_size),
                line_height: px(theme.body_size * theme.line_ratio),
                left: content_left,
                right: page_right,
                top,
                color: theme.text,
                force_bold: false,
                force_mono: false,
                force_color: false,
            }
            .run_mode(source_view, &mut spans, &mut objects)
        }
        BlockKind::FencedCode { .. } | BlockKind::IndentedCode => {
            let code_size = px(theme.body_size * theme.code_ratio);
            let line_height = px(code_size.as_f32() * theme.line_ratio);
            let flow_top = px(top.as_f32() + theme.code_pad);
            let bottom = Flow {
                metrics,
                media,
                theme,
                block,
                block_idx,
                font_size: code_size,
                line_height,
                left: px(page_left.as_f32() + theme.code_pad),
                right: px(page_right.as_f32() - theme.code_pad),
                top: flow_top,
                color: theme.code_text,
                force_bold: false,
                force_mono: true,
                force_color: true,
            }
            .run_mode(source_view, &mut spans, &mut objects);
            let plate = Bounds::new(
                point(
                    px((page_left.as_f32() - theme.code_pad).max(origin_x.as_f32())),
                    top,
                ),
                size(
                    px(page_right.as_f32() - page_left.as_f32() + theme.code_pad * 2.),
                    px(bottom.as_f32() - top.as_f32() + theme.code_pad),
                ),
            );
            decors.push(BlockDecor::Quad {
                bounds: plate,
                color: theme.code_bg,
            });
            box_left = plate.origin.x;
            box_right = px(plate.origin.x.as_f32() + plate.size.width.as_f32());
            px(bottom.as_f32() + theme.code_pad)
        }
        BlockKind::BlockQuote => {
            let content_left = px(page_left.as_f32() + theme.quote_indent);
            let bottom = Flow {
                metrics,
                media,
                theme,
                block,
                block_idx,
                font_size: px(theme.body_size),
                line_height: px(theme.body_size * theme.line_ratio),
                left: content_left,
                right: page_right,
                top,
                color: theme.muted,
                force_bold: false,
                force_mono: false,
                force_color: false,
            }
            .run_mode(source_view, &mut spans, &mut objects);
            decors.push(BlockDecor::Quad {
                bounds: Bounds::new(
                    point(page_left, top),
                    size(
                        px(theme.quote_bar_w),
                        px((bottom.as_f32() - top.as_f32()).max(1.)),
                    ),
                ),
                color: theme.quote_bar,
            });
            bottom
        }
        BlockKind::Rule => {
            let bar_top = px(top.as_f32() + 6.);
            decors.push(BlockDecor::Quad {
                bounds: Bounds::new(
                    point(page_left, bar_top),
                    size(
                        px(page_right.as_f32() - page_left.as_f32()),
                        px(theme.rule_thickness),
                    ),
                ),
                color: theme.rule,
            });
            px(bar_top.as_f32() + theme.rule_thickness + 6.)
        }
        BlockKind::Table => {
            if let Some(source) = source_view {
                Flow {
                    metrics,
                    media,
                    theme,
                    block,
                    block_idx,
                    font_size: px(theme.body_size * theme.code_ratio),
                    line_height: px(theme.body_size * theme.code_ratio * theme.line_ratio),
                    left: page_left,
                    right: page_right,
                    top,
                    color: theme.muted,
                    force_bold: false,
                    force_mono: true,
                    force_color: true,
                }
                .run_mode(Some(source), &mut spans, &mut objects)
            } else {
                table_geometry(
                    theme,
                    metrics,
                    block,
                    block_idx,
                    page_left,
                    page_right,
                    top,
                    &mut spans,
                    &mut decors,
                )
            }
        }
        // Block HTML is stripped to its text and inline styles at parse time;
        // block-level HTML/CSS layout is not implemented yet.
        BlockKind::Html => Flow {
            metrics,
            media,
            theme,
            block,
            block_idx,
            font_size: px(theme.body_size),
            line_height: px(theme.body_size * theme.line_ratio),
            left: page_left,
            right: page_right,
            top,
            color: theme.text,
            force_bold: false,
            force_mono: false,
            force_color: false,
        }
        .run_mode(source_view, &mut spans, &mut objects),
    };

    let content_left = spans
        .first()
        .map(|s| s.origin.x)
        .or_else(|| objects.first().map(|o| o.bounds.origin.x))
        .unwrap_or(box_left);
    let line_height = spans
        .first()
        .map_or_else(|| px(theme.body_size * theme.line_ratio), |s| s.line_height);
    BlockGeometry {
        spans,
        objects,
        decors,
        box_left,
        box_right,
        height: px((bottom.as_f32() - top.as_f32()).max(0.)),
        content_left,
        line_height,
    }
}

/// Bytes of one block, taken straight out of the rope.
///
/// Reads only that block instead of copying the whole buffer: with a caret on a
/// 100k-character document the old code cloned 196 KB per frame just to slice one
/// block out of it (findings H1, optimisation 2).
/// Lay out one GFM table: equal-height rows, column widths from the widest cell,
/// a light grid and a tinted header row. Cells are ordinary `DrawSpan`s, so hit
/// testing, selection and IME keep working through the generic span path.
#[allow(clippy::too_many_arguments)]
fn table_geometry(
    theme: &EditorTheme,
    metrics: &dyn Metrics,
    block: &Block,
    block_idx: usize,
    page_left: Pixels,
    page_right: Pixels,
    top: Pixels,
    spans: &mut Vec<DrawSpan>,
    decors: &mut Vec<BlockDecor>,
) -> Pixels {
    let font_size = px(theme.body_size);
    let line_height = px(theme.body_size * theme.line_ratio);
    let pad = px(theme.page_pad * 0.5);
    let cell_pad = px(10.);
    let Some(table) = &block.table else {
        return px(top.as_f32() + line_height.as_f32());
    };

    // Flatten one row into cell texts: every visible non-marker span that starts
    // inside the cell's source range, in source order.
    struct Cell {
        parts: Vec<(String, doc::Style, Range<usize>)>,
        width: Pixels,
    }
    let mut rows: Vec<Vec<Cell>> = Vec::with_capacity(table.rows.len());
    for row in &table.rows {
        let mut out_row = Vec::with_capacity(row.len());
        for cell_range in row {
            let mut parts = Vec::new();
            for span in &block.spans {
                if span.src.start < cell_range.start || span.src.end > cell_range.end {
                    continue;
                }
                if matches!(span.kind, SpanKind::Marker(_) | SpanKind::Object(_)) {
                    continue;
                }
                let text = &block.render[span.render.clone()];
                if text.is_empty() {
                    continue;
                }
                parts.push((text.to_string(), span.style, span.render.clone()));
            }
            if parts.is_empty() {
                // An empty cell still needs its column width; nothing to paint.
            }
            let width = parts.iter().fold(px(0.), |acc, (text, style, _)| {
                px(acc.as_f32() + metrics.advance(text, font_size, *style).as_f32())
            });
            out_row.push(Cell { parts, width });
        }
        rows.push(out_row);
    }

    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return px(top.as_f32() + line_height.as_f32());
    }
    let mut widths = vec![px(0.); cols];
    for row in &rows {
        for (col, cell) in row.iter().enumerate() {
            let needed = px(cell.width.as_f32() + cell_pad.as_f32() * 2.);
            if needed.as_f32() > widths[col].as_f32() {
                widths[col] = needed;
            }
        }
    }
    let available = px((page_right.as_f32() - page_left.as_f32()).max(40.));
    let total: f32 = widths.iter().map(|w| w.as_f32()).sum();
    if total > available.as_f32() && total > 0. {
        let scale = available.as_f32() / total;
        for width in &mut widths {
            *width = px(width.as_f32() * scale);
        }
    }

    let header_height = px(line_height.as_f32() + 4.);
    let mut y = px(top.as_f32() + pad.as_f32());
    let table_top = y;
    for (row_idx, row) in rows.iter().enumerate() {
        let row_height = if row_idx == 0 {
            header_height
        } else {
            line_height
        };
        if row_idx == 0 {
            decors.push(BlockDecor::Quad {
                bounds: Bounds::new(
                    point(page_left, y),
                    size(
                        px(widths.iter().map(|w| w.as_f32()).sum()),
                        px(row_height.as_f32() + pad.as_f32()),
                    ),
                ),
                color: theme.code_bg,
            });
        }
        let mut x = page_left;
        for (col, cell) in row.iter().enumerate() {
            let mut part_x = px(x.as_f32() + cell_pad.as_f32());
            for (text, style, render) in &cell.parts {
                let width = metrics.advance(text, font_size, *style);
                spans.push(DrawSpan {
                    block_idx,
                    span_idx: block
                        .spans
                        .iter()
                        .position(|span| span.render == *render)
                        .unwrap_or(0),
                    text: text.clone(),
                    render: render.clone(),
                    style: *style,
                    font_size,
                    line_height: row_height,
                    origin: point(part_x, px(y.as_f32() + 2.)),
                    width,
                    color: theme.text,
                    underline: false,
                    strike: style.strike,
                    force_bold: row_idx == 0,
                });
                part_x = px(part_x.as_f32() + width.as_f32());
            }
            x = px(x.as_f32() + widths[col].as_f32());
        }
        y = px(y.as_f32() + row_height.as_f32());
    }
    let table_bottom = y;

    // Grid: outer box, one horizontal line per row, one vertical line per column.
    let grid = theme.rule;
    let width = px(widths.iter().map(|w| w.as_f32()).sum());
    decors.push(BlockDecor::Quad {
        bounds: Bounds::new(point(page_left, table_top), size(width, px(1.))),
        color: grid,
    });
    decors.push(BlockDecor::Quad {
        bounds: Bounds::new(
            point(page_left, px(table_bottom.as_f32() - 1.)),
            size(width, px(1.)),
        ),
        color: grid,
    });
    let mut line_y = table_top;
    for (row_idx, row) in rows.iter().enumerate() {
        line_y = px(line_y.as_f32()
            + if row_idx == 0 {
                header_height.as_f32()
            } else {
                line_height.as_f32()
            });
        if row_idx + 1 < rows.len() {
            decors.push(BlockDecor::Quad {
                bounds: Bounds::new(point(page_left, line_y), size(width, px(1.))),
                color: grid,
            });
        }
        let _ = row;
    }
    let mut x = page_left;
    for (col, col_width) in widths.iter().enumerate() {
        decors.push(BlockDecor::Quad {
            bounds: Bounds::new(
                point(x, table_top),
                size(px(1.), px(table_bottom.as_f32() - table_top.as_f32())),
            ),
            color: grid,
        });
        x = px(x.as_f32() + col_width.as_f32());
        if col + 1 == widths.len() {
            decors.push(BlockDecor::Quad {
                bounds: Bounds::new(
                    point(px(x.as_f32() - 1.), table_top),
                    size(px(1.), px(table_bottom.as_f32() - table_top.as_f32())),
                ),
                color: grid,
            });
        }
    }

    px(table_bottom.as_f32() + pad.as_f32())
}

fn source_slice(doc: &Document, src: &Range<usize>) -> String {
    let len = doc.len_bytes();
    let start = src.start.min(len);
    let end = src.end.clamp(start, len);
    doc.rope().slice(start..end).to_string()
}

// ---------------------------------------------------------------- flow

/// Lays one block's visible spans out as lines.
struct Flow<'a> {
    metrics: &'a dyn Metrics,
    media: &'a dyn MediaMeasures,
    theme: &'a EditorTheme,
    block: &'a Block,
    block_idx: usize,
    font_size: Pixels,
    line_height: Pixels,
    left: Pixels,
    right: Pixels,
    top: Pixels,
    color: Rgba,
    force_bold: bool,
    force_mono: bool,
    /// Ignore inline link/code colours (code blocks, quotes, headings keep one
    /// colour but still honour emphasis).
    force_color: bool,
}

enum Seg {
    Text {
        text: String,
        render: Range<usize>,
        span_idx: usize,
        style: doc::Style,
        /// Painted with a squiggle: only the IME composition string is, so a
        /// reader can see which characters are not in the buffer yet.
        underline: bool,
    },
    Object {
        span_idx: usize,
        kind: ObjectKind,
        metrics: ObjectMetrics,
    },
}

/// One block's source projection, with the IME composition's position inside it.
struct SourceView<'a> {
    text: &'a str,
    marked: Option<Range<usize>>,
}

impl Flow<'_> {
    /// Returns the bottom y of the block, rendered projection.
    fn run(&self, out: &mut Vec<DrawSpan>, objects: &mut Vec<ObjectBox>) -> Pixels {
        self.layout_lines(self.logical_lines(), out, objects)
    }

    /// Returns the bottom y of the block; `source` switches the flowed text from
    /// the render projection to the block's raw Markdown source (plus a live IME
    /// composition, when there is one).
    ///
    /// Both projections share every geometric decision below (line height,
    /// wrapping, pen advance), which is what keeps a caret's x stable across the
    /// switch and lets one hit-test implementation serve both.
    fn run_mode(
        &self,
        source: Option<&SourceView<'_>>,
        out: &mut Vec<DrawSpan>,
        objects: &mut Vec<ObjectBox>,
    ) -> Pixels {
        match source {
            Some(view) => {
                self.layout_lines(source_lines(view.text, view.marked.clone()), out, objects)
            }
            None => self.run(out, objects),
        }
    }

    fn layout_lines(
        &self,
        lines: Vec<Vec<Seg>>,
        out: &mut Vec<DrawSpan>,
        objects: &mut Vec<ObjectBox>,
    ) -> Pixels {
        let media_margin = px(12.);
        let content_width = px((self.right.as_f32() - self.left.as_f32()).max(1.));

        // One line height for the whole block, raised by the tallest inline
        // object. A block has exactly one line_height in this editor, so making
        // one line taller must make all of them taller; that keeps caret,
        // selection and IME geometry on the existing `line * line_height` model.
        let mut line_height = self.line_height;
        let mut baseline_ascent = self.metrics.ascent(self.font_size, doc::Style::PLAIN);
        for line in &lines {
            for seg in line {
                if let Seg::Object { metrics, .. } = seg {
                    line_height = px(line_height.as_f32().max(metrics.height.as_f32()));
                    baseline_ascent = px(baseline_ascent.as_f32().max(metrics.ascent.as_f32()));
                    if metrics.display {
                        line_height = px(line_height
                            .as_f32()
                            .max(metrics.height.as_f32() + media_margin.as_f32() * 2.));
                    }
                }
            }
        }
        let text_ascent = self.metrics.ascent(self.font_size, doc::Style::PLAIN);
        let text_top = px((baseline_ascent.as_f32() - text_ascent.as_f32()).max(0.));

        let object_count = lines
            .iter()
            .flatten()
            .filter(|seg| matches!(seg, Seg::Object { .. }))
            .count();
        let text_count = lines
            .iter()
            .flatten()
            .filter(|seg| matches!(seg, Seg::Text { text, .. } if !text.is_empty()))
            .count();
        let standalone_object = object_count == 1 && text_count == 0;

        let mut y = self.top;
        for line in lines {
            let mut pen = self.left;
            let mut first_on_line = true;
            let mut advanced_for_object = false;
            for seg in &line {
                match seg {
                    Seg::Text {
                        text,
                        render,
                        span_idx,
                        style,
                        underline,
                    } => {
                        let style = self.span_style(*style);
                        let width = self.metrics.advance(text, self.font_size, style);
                        if pen.as_f32() + width.as_f32() > self.right.as_f32() && !first_on_line {
                            y = px(y.as_f32() + line_height.as_f32());
                            pen = self.left;
                        }
                        let (color, underline_style) = self.span_colors(style);
                        out.push(DrawSpan {
                            block_idx: self.block_idx,
                            span_idx: *span_idx,
                            text: text.clone(),
                            render: render.clone(),
                            style,
                            font_size: self.font_size,
                            line_height,
                            origin: point(pen, px(y.as_f32() + text_top.as_f32())),
                            width,
                            color,
                            underline: underline_style || *underline,
                            strike: style.strike,
                            force_bold: self.force_bold,
                        });
                        pen = px(pen.as_f32() + width.as_f32());
                        first_on_line = false;
                    }
                    Seg::Object {
                        span_idx,
                        kind,
                        metrics,
                    } => {
                        let mut width = metrics.width;
                        let mut height = metrics.height;
                        if width.as_f32() > content_width.as_f32() {
                            let scale = content_width.as_f32() / width.as_f32();
                            width = content_width;
                            height = px(height.as_f32() * scale);
                        }
                        if metrics.display {
                            if !first_on_line {
                                y = px(y.as_f32() + line_height.as_f32());
                            }
                            let x = px(self.left.as_f32()
                                + (content_width.as_f32() - width.as_f32()) * 0.5);
                            let top = px(y.as_f32() + media_margin.as_f32());
                            objects.push(ObjectBox {
                                span_idx: *span_idx,
                                kind: kind.clone(),
                                font_size: self.font_size,
                                render: self.block.spans[*span_idx].render.clone(),
                                source: self.block.spans[*span_idx].src.clone(),
                                bounds: Bounds::new(point(x, top), size(width, height)),
                                baseline: px(top.as_f32() + metrics.ascent.as_f32()),
                                display: true,
                            });
                            y = px(top.as_f32() + height.as_f32() + media_margin.as_f32());
                            pen = self.left;
                            first_on_line = true;
                            advanced_for_object = true;
                        } else if standalone_object {
                            let x = px(self.left.as_f32()
                                + (content_width.as_f32() - width.as_f32()) * 0.5);
                            let top = y;
                            objects.push(ObjectBox {
                                span_idx: *span_idx,
                                kind: kind.clone(),
                                font_size: self.font_size,
                                render: self.block.spans[*span_idx].render.clone(),
                                source: self.block.spans[*span_idx].src.clone(),
                                bounds: Bounds::new(point(x, top), size(width, height)),
                                baseline: px(top.as_f32() + metrics.ascent.as_f32()),
                                display: false,
                            });
                            y = px(top.as_f32() + height.as_f32());
                            pen = self.left;
                            first_on_line = true;
                            advanced_for_object = true;
                        } else {
                            if pen.as_f32() + width.as_f32() > self.right.as_f32() && !first_on_line
                            {
                                y = px(y.as_f32() + line_height.as_f32());
                                pen = self.left;
                            }
                            let baseline = px(y.as_f32() + baseline_ascent.as_f32());
                            let top = px(baseline.as_f32() - metrics.ascent.as_f32());
                            objects.push(ObjectBox {
                                span_idx: *span_idx,
                                kind: kind.clone(),
                                font_size: self.font_size,
                                render: self.block.spans[*span_idx].render.clone(),
                                source: self.block.spans[*span_idx].src.clone(),
                                bounds: Bounds::new(point(pen, top), size(width, height)),
                                baseline,
                                display: false,
                            });
                            pen = px(pen.as_f32() + width.as_f32());
                            first_on_line = false;
                        }
                    }
                }
            }
            if !advanced_for_object {
                y = px(y.as_f32() + line_height.as_f32());
            }
        }
        y
    }

    fn span_style(&self, style: doc::Style) -> doc::Style {
        doc::Style {
            bold: style.bold || self.force_bold,
            italic: style.italic,
            code: style.code || self.force_mono,
            link: style.link,
            strike: style.strike,
        }
    }

    fn span_colors(&self, style: doc::Style) -> (Rgba, bool) {
        if self.force_color {
            return (self.color, false);
        }
        if style.link {
            (self.theme.link, true)
        } else if style.code {
            (self.theme.code_text, false)
        } else {
            (self.color, false)
        }
    }

    /// Splits the visible spans into logical lines on `\n`.
    ///
    /// This is trap 1 of the probe report: `shape_line` asserts on newlines, so
    /// every newline has to become a line break here. `SpanKind::Marker` spans
    /// are skipped, which is trap 2: they never advance the pen.
    fn logical_lines(&self) -> Vec<Vec<Seg>> {
        let mut lines: Vec<Vec<Seg>> = vec![Vec::new()];
        for (span_idx, span) in self.block.spans.iter().enumerate() {
            if !span.is_visible() || matches!(span.kind, SpanKind::Marker(_)) {
                continue;
            }
            if let SpanKind::Object(kind) = &span.kind {
                let metrics = self.media.measure(kind, self.font_size);
                lines.last_mut().expect("line buffer").push(Seg::Object {
                    span_idx,
                    kind: kind.clone(),
                    metrics,
                });
                continue;
            }
            let text = &self.block.render[span.render.clone()];
            let parts: Vec<&str> = text.split('\n').collect();
            let mut offset = span.render.start;
            for (i, part) in parts.iter().enumerate() {
                if !part.is_empty() {
                    lines.last_mut().expect("line buffer").push(Seg::Text {
                        text: (*part).to_string(),
                        render: offset..offset + part.len(),
                        span_idx,
                        style: span.style,
                        underline: false,
                    });
                }
                offset += part.len() + 1;
                if i + 1 < parts.len() {
                    lines.push(Vec::new());
                }
            }
        }
        // A trailing newline terminates the last line, it does not open a new one.
        if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines
    }
}

/// Splits a block's raw Markdown source into one [`Seg`] per line.
///
/// `render` byte ranges stay relative to the source slice, because that slice is
/// exactly what a source-mode [`BlockBox::display_text`] holds: `render_byte`
/// therefore means "offset into the painted text" in both projections.
///
/// The source carries no span table, so every segment reports [`SOURCE_SPAN`] and
/// plain styling — Markdown markers are *text* here, which is the whole point.
///
/// `marked` is the byte range of a live IME composition inside `text`. It is cut
/// out as its own segment (and only it) so the composition string is the one
/// underlined run on screen while the buffer stays untouched.
fn source_lines(text: &str, marked: Option<Range<usize>>) -> Vec<Vec<Seg>> {
    let mut lines: Vec<Vec<Seg>> = vec![Vec::new()];
    let mut offset = 0usize;
    let parts: Vec<&str> = text.split('\n').collect();
    for (i, part) in parts.iter().enumerate() {
        let line_start = offset;
        let line_end = offset + part.len();
        if !part.is_empty() {
            // Cut points inside this line: the line ends plus the composition's
            // edges, so the composition becomes a segment of its own.
            let mut cuts = vec![line_start, line_end];
            if let Some(m) = &marked {
                for edge in [m.start, m.end] {
                    if edge > line_start && edge < line_end {
                        cuts.push(edge);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            for pair in cuts.windows(2) {
                let (from, to) = (pair[0], pair[1]);
                if from >= to {
                    continue;
                }
                let underline = marked
                    .as_ref()
                    .is_some_and(|m| from >= m.start && to <= m.end);
                lines.last_mut().expect("line buffer").push(Seg::Text {
                    text: part[from - line_start..to - line_start].to_string(),
                    render: from..to,
                    span_idx: SOURCE_SPAN,
                    style: doc::Style::PLAIN,
                    underline,
                });
            }
        }
        offset += part.len() + 1;
        if i + 1 < parts.len() {
            lines.push(Vec::new());
        }
    }
    if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

// ---------------------------------------------------------------- fingerprint

/// A hash of every block's geometry, so a cached layout can be compared with an
/// uncached one and a stale cache hit cannot hide behind "it looked fine".
///
/// It covers the things a wrong cache key would break: block boxes, fragment
/// origins and widths, the painted text, which projection each block is in, and
/// the decorations.
pub fn geometry_fingerprint(layout: &DocLayout) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    layout.blocks.len().hash(&mut hasher);
    layout.size.height.as_f32().to_bits().hash(&mut hasher);
    for block in &layout.blocks {
        block.block_idx.hash(&mut hasher);
        (block.bounds.origin.x.as_f32().to_bits()).hash(&mut hasher);
        (block.bounds.origin.y.as_f32().to_bits()).hash(&mut hasher);
        (block.bounds.size.width.as_f32().to_bits()).hash(&mut hasher);
        (block.bounds.size.height.as_f32().to_bits()).hash(&mut hasher);
        format!("{:?}", block.kind).hash(&mut hasher);
        block.display_text().hash(&mut hasher);
        block.src.start.hash(&mut hasher);
        block.src.end.hash(&mut hasher);
        block.spans.len().hash(&mut hasher);
        for span in &block.spans {
            span.render.start.hash(&mut hasher);
            span.render.end.hash(&mut hasher);
            span.text.hash(&mut hasher);
            (span.origin.x.as_f32().to_bits()).hash(&mut hasher);
            (span.origin.y.as_f32().to_bits()).hash(&mut hasher);
            (span.width.as_f32().to_bits()).hash(&mut hasher);
        }
        block.decors.len().hash(&mut hasher);
        for decor in &block.decors {
            match decor {
                BlockDecor::Quad { bounds, .. } => {
                    (bounds.origin.x.as_f32().to_bits()).hash(&mut hasher);
                    (bounds.origin.y.as_f32().to_bits()).hash(&mut hasher);
                    (bounds.size.height.as_f32().to_bits()).hash(&mut hasher);
                }
                BlockDecor::Text { text, origin, .. } => {
                    text.hash(&mut hasher);
                    (origin.x.as_f32().to_bits()).hash(&mut hasher);
                    (origin.y.as_f32().to_bits()).hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    // No glob import of gpui here: it exports a `test` module, which would shadow
    // the `#[test]` attribute (the expansion recurses into that module).
    use std::sync::Arc;

    use gpui_kit::gpui::{point, px};

    use super::{
        ApproxMetrics, BlockCache, BlockMode, DocLayout, EditorTheme, LayoutMode, MediaMeasures,
        Metrics, ObjectMetrics, Viewport,
    };
    use typors_core::doc::{BlockKind, Document, ObjectKind};

    /// A document with one heading, one blank line and one paragraph: enough for
    /// every projection question group 4 asks, and independent of the sample
    /// generator.
    const DOC: &str = "# 标题\n\n正文一段\n";

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

    fn painted(layout: &DocLayout, block_idx: usize) -> String {
        layout.blocks[block_idx]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect()
    }

    #[test]
    fn source_projection_paints_markdown_markers_for_one_block_only() {
        let (doc, rendered) = layout(DOC, LayoutMode::RENDERED);
        let (_, source) = layout(DOC, LayoutMode::source(0));

        assert_eq!(doc.blocks().len(), 3, "heading, blank line, paragraph");
        assert_eq!(rendered.blocks[0].mode, BlockMode::Rendered);
        assert!(!painted(&rendered, 0).contains('#'));
        assert!(rendered.checks(&doc).iter().all(|(_, ok, _)| *ok));

        assert_eq!(source.blocks[0].mode, BlockMode::Source);
        assert!(
            painted(&source, 0).starts_with("# "),
            "{:?}",
            painted(&source, 0)
        );
        // The other blocks keep their render projection, marker-free.
        for block in source.blocks.iter().skip(1) {
            assert_eq!(block.mode, BlockMode::Rendered);
            assert_eq!(block.display_text(), doc.blocks()[block.block_idx].render);
        }
        assert!(
            source
                .checks(&doc)
                .iter()
                .all(|(_, ok, detail)| { *ok || panic!("check failed: {detail}") })
        );
    }

    #[test]
    fn source_projection_caret_and_hit_round_trip() {
        let (doc, source) = layout(DOC, LayoutMode::source(0));
        let block = &source.blocks[0];
        let text = block.display_text().to_string();
        let mut boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        boundaries.push(text.len());

        let mut unpainted = 0usize;
        for local in boundaries {
            if !block
                .spans
                .iter()
                .any(|s| s.render.start <= local && local <= s.render.end)
            {
                unpainted += 1;
                continue;
            }
            let rect = source.caret_bounds(0, local).expect("caret rect");
            let probe = point(
                px(rect.origin.x.as_f32() + 1.),
                px(rect.origin.y.as_f32() + rect.size.height.as_f32() * 0.5),
            );
            let hit = source.hit(probe).expect("hit");
            assert_eq!(hit.block_idx, 0);
            assert_eq!(
                source.src_for_display_byte(&doc, hit.block_idx, hit.render_byte),
                Some(block.src.start + local),
                "byte {local}"
            );
        }
        assert_eq!(
            unpainted, 1,
            "only the trailing newline has no painted position"
        );
    }

    #[test]
    fn block_cache_reuses_geometry_without_changing_it() {
        let doc = Document::parse(
            "# 标题\n\n前**粗体**中`代码`后\n\n- 列表一\n- 列表二\n\n```\ncode 行\n```\n\n> 引用\n",
        );
        let theme = EditorTheme::default();
        let metrics: Arc<dyn Metrics> = Arc::new(ApproxMetrics);
        let mut cache = BlockCache::new();

        let cold = DocLayout::layout_mode_marked_cached(
            &doc,
            &theme,
            metrics.clone(),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            Some(&mut cache),
        );
        let ((m_hits, m_misses), cold_fingerprint) =
            (cache.stats(), super::geometry_fingerprint(&cold));
        assert_eq!((m_hits, m_misses), (0, doc.blocks().len() as u64));

        let warm = DocLayout::layout_mode_marked_cached(
            &doc,
            &theme,
            metrics,
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            Some(&mut cache),
        );
        let (w_hits, w_misses) = cache.stats();
        assert_eq!(
            (w_hits - m_hits, w_misses - m_misses),
            (doc.blocks().len() as u64, 0),
            "the second pass must be all hits"
        );
        assert_eq!(
            super::geometry_fingerprint(&warm),
            cold_fingerprint,
            "a cache hit must reproduce the geometry exactly"
        );
    }

    #[test]
    fn block_cache_is_dropped_when_the_buffer_changes() {
        let mut doc = Document::parse(DOC);
        let theme = EditorTheme::default();
        let metrics: Arc<dyn Metrics> = Arc::new(ApproxMetrics);
        let mut cache = BlockCache::new();
        let render = |doc: &Document, cache: &mut BlockCache| {
            DocLayout::layout_mode_marked_cached(
                doc,
                &theme,
                metrics.clone(),
                point(px(0.), px(0.)),
                px(600.),
                LayoutMode::RENDERED,
                None,
                Some(cache),
            )
        };

        let before = render(&doc, &mut cache);
        // Another pass first, so the entries really are there and a stale hit is
        // possible.
        let _ = render(&doc, &mut cache);
        assert_eq!(
            cache.stats().1,
            doc.blocks().len() as u64,
            "one cold pass only"
        );

        // An edit bumps `Document::revision`, which is the cache stamp: the layout
        // must be recomputed, not re-used.
        let at = doc.text().find("正文").expect("the paragraph");
        doc.apply_edit(at..at + "正文".len(), "改过的正文，比原来长一些");
        let after = render(&doc, &mut cache);
        // `prepare` clears the cache *and* its counters, so a non-zero hit count
        // here would be the stale hit this test is looking for.
        assert_eq!(
            cache.stats(),
            (0, doc.blocks().len() as u64),
            "the edit must invalidate the whole cache"
        );
        let painted: String = after
            .blocks
            .iter()
            .flat_map(|b| b.spans.iter().map(|s| s.text.as_str()))
            .collect();
        assert!(painted.contains("改过的正文"), "{painted:?}");
        assert_ne!(
            super::geometry_fingerprint(&before),
            super::geometry_fingerprint(&after)
        );
    }

    #[test]
    fn block_geometry_is_stored_relative_to_the_block_top() {
        // Two documents whose second block starts at different y must produce the
        // same *relative* fragments: that is what makes the cache translation-only.
        let theme = EditorTheme::default();
        let metrics: Arc<dyn Metrics> = Arc::new(ApproxMetrics);
        let tall = Document::parse("# 标题\n\n正文一段\n");
        let tall_layout = DocLayout::layout_mode_marked(
            &tall,
            &theme,
            metrics.clone(),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
        );
        let para = &tall_layout.blocks[2];
        assert!(!para.spans.is_empty());
        // The fragment's y is the block top, not 0, because `materialize` translated
        // it; the *cache* stores it at 0.
        assert_eq!(para.spans[0].origin.y, para.bounds.origin.y);
    }

    #[test]
    fn viewport_band_is_derived_from_the_scrolled_element_top() {
        // A window 800 tall, the element scrolled up by 1000: the visible band in
        // element-local coordinates is 1000..1800, plus the margin on both sides.
        let band = Viewport::on_screen(-1000., 800., 800.);
        assert_eq!(band.top, 200.);
        assert_eq!(band.bottom, 2600.);

        assert!(
            band.intersects(1000., 1050.),
            "a line at the top of the viewport"
        );
        assert!(band.intersects(2500., 2550.), "inside the bottom margin");
        assert!(!band.intersects(3000., 3050.), "below the margin");
        assert!(!band.intersects(0., 50.), "above the margin");

        // An unscrolled element of a short document shows everything it has.
        let top = Viewport::on_screen(0., 800., 800.);
        assert!(top.intersects(0., 10.));
        assert!(!top.intersects(-2000., -1900.));

        assert!(Viewport::UNBOUNDED.is_unbounded());
        assert!(Viewport::UNBOUNDED.intersects(f32::MIN, f32::MAX));
    }

    fn many_paragraphs(count: usize) -> String {
        (0..count).fold(String::new(), |mut text, index| {
            text.push_str(&format!("paragraph {index} text\n\n"));
            text
        })
    }

    #[test]
    fn windowed_layout_measures_the_viewport_first() {
        let text = many_paragraphs(200);
        let doc = Document::parse(&text);
        let theme = EditorTheme::default();
        let window = Viewport::on_screen(0., 600., 200.);
        let mut layout = DocLayout::layout_windowed_media(
            &doc,
            &theme,
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            &super::NoMedia,
            0,
            None,
            Some(window),
        );
        assert!(layout.measured_blocks > 0);
        assert!(
            layout.estimated_blocks > 0,
            "a 200 paragraph document must leave work for the background pass"
        );
        assert!(layout.blocks[0].measured);
        let far = layout.blocks.len() - 2;
        assert!(!layout.blocks[far].measured);
        assert!(layout.blocks[far].spans.is_empty());
        for block in &layout.blocks {
            let top = block.bounds.origin.y.as_f32();
            if window.intersects(top, top + block.bounds.size.height.as_f32()) {
                assert!(
                    block.measured,
                    "visible block {} is a placeholder",
                    block.block_idx
                );
            }
        }

        let measured = layout.ensure_measured(&doc, &super::NoMedia, 0, Viewport::UNBOUNDED, None);
        assert!(measured > 0);
        assert_eq!(layout.estimated_blocks, 0);
        let full = DocLayout::layout_mode(
            &doc,
            &theme,
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
        );
        assert_eq!(
            super::geometry_fingerprint(&layout),
            super::geometry_fingerprint(&full),
            "a fully measured windowed layout must equal the whole-document one"
        );
    }

    #[test]
    fn measuring_keeps_tops_consistent_and_extent_exact() {
        let text = many_paragraphs(80);
        let doc = Document::parse(&text);
        let theme = EditorTheme::default();
        let window = Viewport::on_screen(0., 400., 100.);
        let mut layout = DocLayout::layout_windowed_media(
            &doc,
            &theme,
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            &super::NoMedia,
            0,
            None,
            Some(window),
        );
        layout.ensure_measured(&doc, &super::NoMedia, 0, window, None);
        // Tops are non-decreasing and the extent reaches past the last block.
        let mut previous = 0.0;
        for block in &layout.blocks {
            let top = block.bounds.origin.y.as_f32();
            assert!(top >= previous, "block {} moved above its predecessor", block.block_idx);
            previous = top;
        }
        let last = layout.blocks.last().expect("a block");
        assert!(layout.size.height.as_f32() >= last.bounds.bottom().as_f32());
        // Measuring the same window again is a no-op.
        let again = layout.ensure_measured(&doc, &super::NoMedia, 0, window, None);
        assert_eq!(again, 0);
    }

    #[test]
    fn block_cache_carries_measurements_into_the_next_frame() {
        let text = many_paragraphs(120);
        let doc = Document::parse(&text);
        let theme = EditorTheme::default();
        let mut cache = BlockCache::new();
        let window = Viewport::on_screen(0., 400., 100.);
        let mut layout = DocLayout::layout_windowed_media(
            &doc,
            &theme,
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            &super::NoMedia,
            0,
            Some(&mut cache),
            Some(window),
        );
        let first = layout.measured_blocks;
        layout.ensure_measured(
            &doc,
            &super::NoMedia,
            0,
            Viewport::on_screen(0., 2000., 100.),
            Some(&mut cache),
        );
        let second = layout.measured_blocks;
        assert!(second > first);
        // The next frame starts with the same cache: what was measured stays
        // exact, so no block's top moves back to an estimate.
        let rebuilt = DocLayout::layout_windowed_media(
            &doc,
            &theme,
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            &super::NoMedia,
            0,
            Some(&mut cache),
            Some(window),
        );
        assert!(rebuilt.measured_blocks >= first);
    }




    #[test]
    fn the_first_enter_reserves_the_caret_line() {
        let mut doc = Document::parse("hello");
        doc.apply_edit(5..5, "\n");
        let mut layout = DocLayout::layout_mode_marked(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::source(0),
            None,
        );
        let before = layout.size.height.as_f32();
        let caret = 6usize;
        let caret_bounds = layout.caret_bounds(0, caret).expect("caret after the newline");
        assert!(
            caret_bounds.origin.y.as_f32() + caret_bounds.size.height.as_f32()
                > layout.blocks[0].bounds.bottom().as_f32(),
            "the caret starts outside the block box (the reported bug)"
        );
        assert!(layout.ensure_caret_line(Some(caret), Some(0)));
        assert!(
            layout.blocks[0].bounds.bottom().as_f32()
                >= caret_bounds.origin.y.as_f32() + caret_bounds.size.height.as_f32(),
            "the block must contain the caret line"
        );
        assert!(layout.size.height.as_f32() > before);
        // Idempotent: a second call has nothing left to add.
        assert!(!layout.ensure_caret_line(Some(caret), Some(0)));
    }

    #[test]
    fn the_caret_line_fix_skips_blank_runs_and_mid_block_carets() {
        // A blank line already draws its own box; the second Enter must not
        // double it.
        let doc = Document::parse("hello\n\n");
        let mut layout = DocLayout::layout_mode_marked(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::source(1),
            None,
        );
        assert!(!layout.ensure_caret_line(Some(7), Some(1)));
        // A caret inside the text has a line already.
        let mut doc = Document::parse("hello");
        doc.apply_edit(5..5, "\n");
        let mut layout = DocLayout::layout_mode_marked(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::source(0),
            None,
        );
        assert!(!layout.ensure_caret_line(Some(3), Some(0)));
        // No source block at all: nothing to do.
        assert!(!layout.ensure_caret_line(Some(6), None));
    }

    #[test]
    fn a_multiline_paragraph_ends_with_one_extra_caret_line() {
        let mut doc = Document::parse("a\nb");
        doc.apply_edit(3..3, "\n");
        let mut layout = DocLayout::layout_mode_marked(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::source(0),
            None,
        );
        let before = layout.blocks[0].bounds.size.height.as_f32();
        assert!(layout.ensure_caret_line(Some(4), Some(0)));
        let after = layout.blocks[0].bounds.size.height.as_f32();
        assert!((after - before - layout.blocks[0].line_height.as_f32()).abs() < 0.5);
        assert!(!layout.ensure_caret_line(Some(4), Some(0)));
    }

    #[test]
    fn source_slice_reads_one_block_out_of_the_rope() {
        let doc = Document::parse(DOC);
        let texts: Vec<String> = doc
            .blocks()
            .iter()
            .map(|b| super::source_slice(&doc, &b.src))
            .collect();
        assert_eq!(texts.concat(), doc.text());
        // And a range past the end is clamped rather than panicking.
        let len = doc.len_bytes();
        assert!(super::source_slice(&doc, &(0..len + 100)).len() <= len + 100);
    }

    #[test]
    fn caret_at_line_starts_and_trailing_newline() {
        let (doc, layout) = layout("a\nb\n", LayoutMode::RENDERED);
        let block = &layout.blocks[0];
        let left = block.content_left;
        let line = block.line_height;

        let (x, y, h) = layout.caret_x(0, 0).expect("start");
        assert_eq!(x, left);
        assert_eq!(y, block.bounds.origin.y);
        assert_eq!(h, line);

        let (end_a, _, _) = layout.caret_x(0, 1).expect("end of first line");
        assert!(end_a > left, "caret before newline is after the text");

        let (start_b, y_b, _) = layout.caret_x(0, 2).expect("start of second line");
        assert_eq!(start_b, left, "line start is the content edge");
        assert!(
            (y_b.as_f32() - (block.bounds.origin.y.as_f32() + line.as_f32())).abs() < 0.5,
            "second line top, got {:.1}",
            y_b.as_f32()
        );

        let (end_x, end_y, end_h) = layout.caret_x(0, 4).expect("end of document");
        assert!(end_x > left || end_y > block.bounds.origin.y);
        assert_eq!(end_h, line);

        let _ = doc;
    }

    #[test]
    fn caret_height_uses_the_owning_line_box() {
        let (_, heading) = layout("# 标题\n", LayoutMode::RENDERED);
        let h = heading.blocks[0].line_height;
        let (_, _, caret_h) = heading.caret_x(0, 0).expect("heading caret");
        assert_eq!(caret_h, h, "heading caret uses the heading line height");
        let bounds = heading.caret_bounds(0, 0).expect("bounds");
        assert_eq!(bounds.size.width, px(2.0));
    }

    #[test]
    fn caret_after_an_atomic_object_uses_its_right_edge() {
        let (_, layout) = layout("前![x](a.png)后\n", LayoutMode::RENDERED);
        let object = &layout.blocks[0].objects[0];
        let (x, y, h) = layout
            .caret_x(0, object.render.end)
            .expect("caret after the object placeholder");
        assert_eq!(x, object.bounds.right());
        assert_eq!(y, object.bounds.top());
        assert_eq!(h, object.bounds.size.height);
    }

    #[test]
    fn empty_document_lays_out_one_caret_line() {
        let (doc, layout) = layout("", LayoutMode::RENDERED);
        assert_eq!(doc.blocks().len(), 1);
        assert_eq!(layout.blocks.len(), 1);
        assert!(layout.blocks[0].bounds.size.height.as_f32() > 0.);
        assert!(layout.blocks[0].spans.is_empty());
        let caret = layout.caret_bounds(0, 0).expect("blank caret");
        assert_eq!(caret.size.width, px(2.0));
        assert!(caret.size.height.as_f32() > 0.);
    }

    #[test]
    fn line_index_covers_every_byte_of_a_multiline_block() {
        let (doc, layout) = layout("a\nb\nc", LayoutMode::RENDERED);
        for byte in 0..=doc.len_bytes() {
            let line = layout.line_index_for_byte(0, byte).expect("line for byte");
            let bounds = layout.line_bounds(0, line).expect("line bounds");
            assert!(bounds.bottom >= bounds.top);
            assert!(bounds.render.start <= bounds.render.end);
        }
    }

    #[test]
    fn caret_paint_two_frames_after_scroll_has_one_caret_at_the_new_bounds() {
        let (_, mut layout) = layout("first\nsecond\nthird", LayoutMode::RENDERED);
        let frame_one = layout.caret_bounds(0, 2).expect("caret frame one");
        // The frame after a scroll uses the translated layout; there is no second
        // caret retained at the old origin.
        layout.translate(point(px(0.), px(-80.)));
        let frame_two = layout.caret_bounds(0, 2).expect("caret frame two");
        assert_ne!(frame_one.origin.y, frame_two.origin.y);
        assert_eq!(frame_one.size, frame_two.size);
        let caret_blocks = layout
            .blocks
            .iter()
            .filter(|block| block.block_idx == 0)
            .count();
        assert_eq!(caret_blocks, 1);
    }

    #[test]
    fn caret_translation_moves_the_only_caret_with_the_document() {
        let (_, mut layout) = layout("a\nb", LayoutMode::RENDERED);
        let old = layout.caret_bounds(0, 2).expect("caret");
        layout.translate(point(px(0.), px(40.)));
        let new = layout.caret_bounds(0, 2).expect("caret after scroll");
        assert!((new.origin.y.as_f32() - old.origin.y.as_f32() - 40.).abs() < 0.01);
        assert_eq!(new.origin.x, old.origin.x);
        assert_eq!(new.size, old.size);
    }

    #[test]
    fn code_block_caret_uses_the_code_line_height() {
        let (_, layout) = layout("```\ncode\n```\n", LayoutMode::RENDERED);
        let block = layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, BlockKind::FencedCode { .. }))
            .expect("fence block");
        let bounds = layout
            .caret_bounds(block.block_idx, 0)
            .expect("caret in code");
        assert_eq!(bounds.size.height, block.line_height);
    }

    #[test]
    fn blank_line_owns_a_caret_line() {
        let (doc, layout) = layout(DOC, LayoutMode::RENDERED);
        let gap = layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Gap && b.spans.is_empty())
            .expect("a blank line between the heading and the paragraph");
        assert!(
            gap.bounds.size.height.as_f32() > 0.,
            "a blank line has height"
        );
        let (_, top, line_height) = layout
            .caret_x(gap.block_idx, 0)
            .expect("caret on a blank line");
        assert_eq!(top, gap.bounds.origin.y);
        assert!(line_height.as_f32() > 0.);

        let mid = px(gap.bounds.origin.y.as_f32() + gap.bounds.size.height.as_f32() * 0.5);
        let hit = layout
            .hit(point(px(gap.bounds.origin.x.as_f32() + 2.), mid))
            .expect("hit on a blank line");
        assert_eq!(hit.block_idx, gap.block_idx);
        assert_eq!(hit.render_byte, 0);
        assert_eq!(
            layout.src_for_display_byte(&doc, hit.block_idx, hit.render_byte),
            Some(gap.src.start)
        );
    }

    /// A run of blank lines is one block with one line box per newline, and the caret
    /// has to walk those line boxes.
    ///
    /// If it were always drawn at the block's top it would stand still while the user
    /// pressed Enter — which is exactly「回车后光标不跟随」: the ribbon does not move,
    /// so no scroll correction can follow it and the new lines fall off the bottom of
    /// the window.
    #[test]
    fn a_caret_inside_a_blank_run_walks_its_line_boxes() {
        let doc = Document::parse("A\n\n\n\n");
        let layout = DocLayout::layout(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
        );
        let gap = layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Gap && b.spans.is_empty())
            .expect("the trailing newlines own one blank block");
        let line = gap.line_height.as_f32();
        let lines = super::blank_line_count(gap);
        assert!(lines >= 3, "the blank run owns {lines} line boxes");

        for index in 0..lines {
            let (x, top, height) = layout
                .caret_x(gap.block_idx, index)
                .expect("caret inside the blank run");
            assert_eq!(x, gap.content_left, "a blank line has no content");
            assert_eq!(height, gap.line_height);
            assert!(
                (top.as_f32() - (gap.bounds.origin.y.as_f32() + index as f32 * line)).abs() < 0.5,
                "line {index}: top {:.1}, expected {:.1}",
                top.as_f32(),
                gap.bounds.origin.y.as_f32() + index as f32 * line
            );
        }

        // Past the end of the run the caret stays on the last line rather than falling
        // off the bottom of the block: the trailing newline terminates the last line,
        // it does not open a new one.
        let (_, last, _) = layout
            .caret_x(gap.block_idx, lines + 4)
            .expect("caret past the end of the run");
        let last_expected = gap.bounds.origin.y.as_f32() + (lines - 1) as f32 * line;
        assert!(
            (last.as_f32() - last_expected).abs() < 0.5,
            "the caret past the run is at {:.1}, expected the last line {last_expected:.1}",
            last.as_f32()
        );

        // The arrow keys walk `line_tops`, so it has to list the same lines.
        let tops = layout.line_tops(gap.block_idx);
        assert_eq!(tops.len(), lines);
        assert_eq!(tops[0], gap.bounds.origin.y);
    }

    /// A fragment-less block that is *not* a blank run (a rule) must keep its caret on
    /// its own single line, so the fix above cannot move it by accident.
    #[test]
    fn a_fragment_less_block_keeps_its_caret_inside_itself() {
        let doc = Document::parse("A\n\n---\n\nB\n");
        let layout = DocLayout::layout(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
        );
        let rule = layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Rule)
            .expect("a rule block");
        let (_, top, _) = layout.caret_x(rule.block_idx, 0).expect("caret on a rule");
        assert_eq!(top, rule.bounds.origin.y);
        let tops = layout.line_tops(rule.block_idx);
        assert!(!tops.is_empty());
        for top in tops {
            assert!(top.as_f32() >= rule.bounds.origin.y.as_f32() - 0.5);
            assert!(top.as_f32() <= rule.bounds.bottom().as_f32() + 0.5);
        }
    }

    /// `blank_line_start` and `blank_line_index` are inverses, which is what makes
    /// `hit` the reverse of `caret_x` inside a blank run.
    #[test]
    fn blank_line_start_and_index_are_inverses() {
        let doc = Document::parse("A\n\n\n\n\n\n");
        let layout = DocLayout::layout(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
        );
        let gap = layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Gap && b.spans.is_empty())
            .expect("a blank run");
        let lines = super::blank_line_count(gap);
        assert!(lines >= 5, "the run owns {lines} line boxes");
        for line in 0..lines {
            let start = super::blank_line_start(gap, line);
            assert_eq!(
                super::blank_line_index(gap, start),
                line,
                "line {line} starts at byte {start}"
            );
            assert!(
                gap.display_text().is_char_boundary(start),
                "line {line} starts mid-character at {start}"
            );
        }
        // The first line starts at byte 0; the last starts before the end.
        assert_eq!(super::blank_line_start(gap, 0), 0);
        assert!(super::blank_line_start(gap, lines - 1) < gap.display_text().len());
        // Asking past the end clamps to the end instead of panicking.
        assert_eq!(
            super::blank_line_start(gap, lines + 10),
            gap.display_text().len()
        );
    }

    /// A click inside a run of blank lines lands on the line it was made in, and the
    /// caret for that byte sits on the same line box.
    ///
    /// Before this, `hit` returned `render_byte: 0` for every fragment-less block, so
    /// clicking the 5th blank line put the caret back on the 1st.
    #[test]
    fn hit_inside_a_blank_run_lands_on_the_clicked_line() {
        let doc = Document::parse("A\n\n\n\n\n\n");
        let layout = DocLayout::layout(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
        );
        let gap = layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Gap && b.spans.is_empty())
            .expect("a blank run");
        let tops = layout.line_tops(gap.block_idx);
        let lines = tops.len();
        assert!(lines >= 5, "the run owns {lines} line boxes");
        let line_height = gap.line_height.as_f32();
        let mut bytes = Vec::new();
        for (line, top) in tops.iter().enumerate() {
            // x is irrelevant on a blank line: probe the left edge and far right.
            for x in [gap.content_left, px(gap.content_left.as_f32() + 4000.)] {
                let hit = layout
                    .hit(point(x, px(top.as_f32() + line_height / 2.)))
                    .expect("a hit inside the run");
                assert_eq!(hit.block_idx, gap.block_idx);
                assert_eq!(
                    hit.render_byte,
                    super::blank_line_start(gap, line),
                    "line {line} at x={:.1}",
                    x.as_f32()
                );
                let (_, caret_top, _) = layout
                    .caret_x(hit.block_idx, hit.render_byte)
                    .expect("a caret in the run");
                assert!((caret_top.as_f32() - top.as_f32()).abs() < 0.5);
            }
            bytes.push(super::blank_line_start(gap, line));
        }
        // One line per press, in order: the 5th line's byte is past the 1st's.
        assert!(
            bytes.windows(2).all(|pair| pair[0] < pair[1]),
            "line starts are not increasing: {bytes:?}"
        );
    }

    /// The block's own edges clamp to its own lines, like every other `hit`.
    #[test]
    fn hit_clamps_at_the_edges_of_a_blank_run() {
        let doc = Document::parse("A\n\n\n\n\n\n");
        let layout = DocLayout::layout(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
        );
        let gap = layout
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Gap && b.spans.is_empty())
            .expect("a blank run");
        let lines = layout.line_tops(gap.block_idx).len();
        let x = px(gap.content_left.as_f32() + 2.);

        let top_edge = layout
            .hit(point(x, gap.bounds.origin.y))
            .expect("a hit on the block's top edge");
        assert_eq!(top_edge.render_byte, 0);

        let bottom_edge = layout
            .hit(point(x, px(gap.bounds.bottom().as_f32() - 0.5)))
            .expect("a hit on the block's bottom edge");
        assert_eq!(
            bottom_edge.render_byte,
            super::blank_line_start(gap, lines - 1)
        );

        // Nothing outside the block's lines is ever reported.
        for hit in [top_edge, bottom_edge] {
            assert!(hit.render_byte <= gap.display_text().len());
        }
    }

    struct FakeMedia;

    impl MediaMeasures for FakeMedia {
        fn measure(&self, kind: &ObjectKind, _font_size: gpui_kit::gpui::Pixels) -> ObjectMetrics {
            match kind {
                ObjectKind::Image { .. } => ObjectMetrics {
                    width: px(200.),
                    height: px(100.),
                    ascent: px(75.),
                    descent: px(25.),
                    display: false,
                },
                ObjectKind::Math { display: true, .. } => ObjectMetrics {
                    width: px(120.),
                    height: px(60.),
                    ascent: px(45.),
                    descent: px(15.),
                    display: true,
                },
                ObjectKind::Math { display: false, .. } => ObjectMetrics {
                    width: px(40.),
                    height: px(20.),
                    ascent: px(15.),
                    descent: px(5.),
                    display: false,
                },
                ObjectKind::Diagram { .. } => ObjectMetrics {
                    width: px(480.),
                    height: px(240.),
                    ascent: px(180.),
                    descent: px(60.),
                    display: true,
                },
                ObjectKind::Chart { .. } => ObjectMetrics {
                    width: px(560.),
                    height: px(340.),
                    ascent: px(255.),
                    descent: px(85.),
                    display: true,
                },
            }
        }
    }

    fn media_layout(text: &str) -> (Document, DocLayout) {
        let doc = Document::parse(text);
        let layout = DocLayout::layout_mode_marked_cached_media(
            &doc,
            &EditorTheme::default(),
            Arc::new(ApproxMetrics),
            point(px(0.), px(0.)),
            px(600.),
            LayoutMode::RENDERED,
            None,
            &FakeMedia,
            0,
            None,
        );
        (doc, layout)
    }

    #[test]
    fn inline_math_shares_the_text_baseline() {
        let (_doc, layout) = media_layout("前 $x^2$ 后\n");
        let block = &layout.blocks[0];
        assert_eq!(block.spans.len(), 2);
        assert_eq!(block.objects.len(), 1);
        let object = &block.objects[0];
        let text = &block.spans[0];
        let text_baseline =
            px(text.origin.y.as_f32()
                + layout.metrics().ascent(text.font_size, text.style).as_f32());
        assert!(
            (object.baseline.as_f32() - text_baseline.as_f32()).abs() < 0.01,
            "object baseline {:?} != text baseline {:?}",
            object.baseline,
            text_baseline
        );
        assert!(block.line_height.as_f32() >= object.bounds.size.height.as_f32());
    }

    #[test]
    fn display_math_gets_a_centered_line_and_standalone_image_is_centered() {
        let (_doc, layout) = media_layout("$$\\frac{a}{b}$$\n\n![图](a.png)\n");
        let math = layout
            .blocks
            .iter()
            .find(|b| b.objects.iter().any(|o| o.display))
            .expect("display math block");
        let image = layout
            .blocks
            .iter()
            .find(|b| {
                b.objects
                    .iter()
                    .any(|o| matches!(o.kind, ObjectKind::Image { .. }))
            })
            .expect("image block");
        assert_eq!(math.objects.len(), 1);
        assert!(math.objects[0].display);
        let math_object = &math.objects[0];
        let math_center =
            math_object.bounds.origin.x.as_f32() + math_object.bounds.size.width.as_f32() / 2.;
        let block_center = math.bounds.origin.x.as_f32() + math.bounds.size.width.as_f32() / 2.;
        assert!((math_center - block_center).abs() < 1.0);

        assert_eq!(image.objects.len(), 1);
        let image_object = &image.objects[0];
        assert_eq!(image_object.bounds.size.width.as_f32(), 200.);
        let image_center =
            image_object.bounds.origin.x.as_f32() + image_object.bounds.size.width.as_f32() / 2.;
        let image_block_center =
            image.bounds.origin.x.as_f32() + image.bounds.size.width.as_f32() / 2.;
        assert!((image_center - image_block_center).abs() < 1.0);
        assert!(image.bounds.size.height.as_f32() >= 100.);
    }

    #[test]
    fn object_hit_reports_the_clicked_half() {
        let (_doc, layout) = media_layout("![图](a.png)\n");
        let object = &layout.blocks[0].objects[0];
        let y = object.bounds.origin.y.as_f32() + object.bounds.size.height.as_f32() / 2.;
        let left = layout
            .object_at(point(object.bounds.origin.x, px(y)))
            .expect("left half");
        assert!(!left.right_half);
        let right = layout
            .object_at(point(px(object.bounds.right().as_f32() - 1.), px(y)))
            .expect("right half");
        assert!(right.right_half);
        assert_eq!(left.source, object.source);
    }

    #[test]
    fn media_generation_invalidates_the_block_cache() {
        let doc = Document::parse("![图](a.png)\n");
        let theme = EditorTheme::default();
        let mut cache = BlockCache::new();
        let run = |cache: &mut BlockCache, generation: u64| {
            DocLayout::layout_mode_marked_cached_media(
                &doc,
                &theme,
                Arc::new(ApproxMetrics),
                point(px(0.), px(0.)),
                px(600.),
                LayoutMode::RENDERED,
                None,
                &FakeMedia,
                generation,
                Some(cache),
            )
        };
        run(&mut cache, 0);
        let (hits, _) = cache.stats();
        run(&mut cache, 0);
        assert!(cache.stats().0 > hits, "same generation must hit");
        run(&mut cache, 1);
        assert!(
            cache.stats().1 > 0,
            "a new media generation must miss at least once"
        );
    }
}
