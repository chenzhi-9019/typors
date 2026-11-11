//! Hybrid-editing core of the M0 spike: one `ropey` buffer, a block AST, and a
//! per-block span table that maps source offsets to render offsets and back.
//!
//! Invariants (asserted by the tests at the bottom of this module):
//!
//! 1. `blocks` tile `[0, len_bytes())` in source order, and `spans` tile each
//!    block, so every source byte belongs to exactly one block and one span.
//! 2. A visible span is byte-identical to its source slice (`SpanKind::Text`),
//!    so an offset inside it is the same number on both sides of the mapping.
//! 3. Every offset produced by this API lands on a UTF-8 char boundary.
//! 4. `src_to_render` / `render_to_src` round-trip exactly for every char
//!    boundary offset; a zero-width marker run keeps its own local offsets, so
//!    nothing inside it is lost either.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ropey::Rope;

// ---------------------------------------------------------------- model

/// Inline style carried by a span. A renderer only has to read this.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
    /// `~~struck through~~`; painted as a line over the glyphs.
    pub strike: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        bold: false,
        italic: false,
        code: false,
        link: false,
        strike: false,
    };
    pub const CODE: Style = Style {
        bold: false,
        italic: false,
        code: true,
        link: false,
        strike: false,
    };
}

/// What kind of block a source range is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Heading {
        level: u8,
    },
    Paragraph,
    ListItem {
        ordered: bool,
        level: usize,
    },
    FencedCode {
        info: String,
    },
    IndentedCode,
    BlockQuote,
    Rule,
    Html,
    /// A GFM table. The cell source ranges are in [`Block::table`].
    Table,
    /// Source bytes owned by no Markdown block: blank lines and stray text.
    Gap,
}

/// Which Markdown token a zero-width marker span stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkerKind {
    HeadingHash,
    ListBullet,
    TaskBox,
    QuoteMarker,
    EmphasisDelim,
    InlineCodeDelim,
    CodeFence,
    LinkSyntax,
    LineBreak,
    Other,
}

/// A rich-content object: an inline or block-level image/formula that the render
/// projection paints as one atomic box instead of as text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ObjectKind {
    Image {
        /// The link destination exactly as written (relative path, URL or data URI).
        destination: String,
        /// Optional Markdown title.
        title: String,
        /// The alt text, with inner Markdown markers stripped by the parser.
        alt: String,
    },
    Math {
        /// TeX source, without the `$`/`$$` delimiters.
        tex: String,
        /// `$$...$$` (true) versus `$...$` (false).
        display: bool,
    },
    /// A fenced ```mermaid block, rendered natively.
    Diagram { source: String },
    /// A fenced ```chart block, rendered natively.
    Chart { source: String },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpanKind {
    /// Visible and byte-identical to its source slice.
    Text,
    /// Hidden, zero render width (`span.render.start == span.render.end`).
    Marker(MarkerKind),
    /// Visible but not byte-identical to source: escapes, entities, raw HTML,
    /// hard breaks, normalised inline code. Offsets inside it are clamped.
    Degraded,
    /// One atomic object. `render` is exactly the U+FFFC placeholder; all source
    /// offsets inside it collapse onto that placeholder in the visible projection,
    /// while [`RenderPos`] keeps the exact offset so the lossless mapping holds.
    Object(ObjectKind),
}

/// One inline fragment (or decoration) of a block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    /// Absolute byte range in the document.
    pub src: Range<usize>,
    /// Byte range inside [`Block::render`].
    pub render: Range<usize>,
    pub kind: SpanKind,
    pub style: Style,
}

impl Span {
    pub fn is_visible(&self) -> bool {
        self.render.start < self.render.end
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Block {
    pub kind: BlockKind,
    /// Absolute byte range in the document, line aligned.
    pub src: Range<usize>,
    /// Marker-stripped text of the block.
    pub render: String,
    pub spans: Vec<Span>,
    /// Cell source ranges for [`BlockKind::Table`], in row-major order.
    pub table: Option<TableData>,
}

/// One GFM table: rows of cell source ranges, in row-major order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableData {
    pub rows: Vec<Vec<Range<usize>>>,
}

/// A position in the render projection of a block.
///
/// `offset_in_inline` is an offset inside `span`: identical to the offset inside
/// [`Block::render`] for a visible span, and the local offset inside the
/// zero-width marker run for a [`SpanKind::Marker`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct RenderPos {
    pub block_idx: usize,
    pub inline_idx: usize,
    pub offset_in_inline: usize,
}

// ---------------------------------------------------------------- document

#[derive(Clone, Debug)]
pub struct Document {
    rope: Rope,
    blocks: Vec<Block>,
    revision: u64,
}

impl Document {
    pub fn parse(text: &str) -> Self {
        Document {
            rope: Rope::from_str(text),
            blocks: parse_blocks(text),
            revision: 0,
        }
    }

    /// Replace `src` with `replace_with` in the single buffer and reparse.
    /// (The later groups drive this from the UI; the mapping must survive it.)
    pub fn apply_edit(&mut self, src: Range<usize>, replace_with: &str) {
        let start = self.snap_floor(src.start);
        let end = self.snap_ceil(src.end.max(start));
        self.rope.remove(start..end);
        if !replace_with.is_empty() {
            self.rope.insert(start, replace_with);
        }
        self.revision += 1;
        let text = self.rope.to_string();
        self.blocks = parse_blocks(&text);
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }
    pub fn rope(&self) -> &Rope {
        &self.rope
    }
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn len_bytes(&self) -> usize {
        self.rope.len()
    }

    pub fn is_char_boundary(&self, byte: usize) -> bool {
        byte <= self.len_bytes() && self.rope.is_char_boundary(byte)
    }

    /// Largest char boundary `<= byte`.
    pub fn snap_floor(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.len_bytes());
        while byte > 0 && !self.rope.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    /// Smallest char boundary `>= byte`.
    pub fn snap_ceil(&self, byte: usize) -> usize {
        let len = self.len_bytes();
        let mut byte = byte.min(len);
        while byte < len && !self.rope.is_char_boundary(byte) {
            byte += 1;
        }
        byte
    }

    /// Index of the block owning `src`. Blocks tile the document, so this is
    /// total for `src <= len_bytes()`; the end offset belongs to the last block.
    pub fn block_at_src(&self, src: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        let idx = self
            .blocks
            .partition_point(|b| b.src.start <= src)
            .saturating_sub(1);
        Some(idx.min(self.blocks.len() - 1))
    }

    /// True when this byte belongs to a zero-width Markdown marker. Such bytes
    /// deliberately collapse onto one visible position, so source round-tripping
    /// is many-to-one there.
    pub fn is_marker_byte(&self, src: usize) -> bool {
        let Some(idx) = self.block_at_src(src) else {
            return false;
        };
        self.blocks[idx]
            .spans
            .iter()
            .any(|span| matches!(span.kind, SpanKind::Marker(_)) && span.src.contains(&src))
    }

    /// True when the caret is on or immediately next to an inline delimiter
    /// (`**`, `*`, `` ` ``, link brackets). Typora reveals those delimiters only
    /// at the caret, so the block temporarily uses the source projection.
    pub fn caret_touches_inline_marker(&self, caret: usize) -> bool {
        let Some(idx) = self.block_at_src(caret) else {
            return false;
        };
        let block = &self.blocks[idx];
        block.spans.iter().any(|span| {
            let inline = matches!(
                span.kind,
                SpanKind::Marker(
                    MarkerKind::EmphasisDelim
                        | MarkerKind::InlineCodeDelim
                        | MarkerKind::LinkSyntax
                )
            );
            if !inline {
                return false;
            }
            let start = span.src.start;
            let end = span.src.end;
            caret == start
                || caret == end
                || caret == self.snap_floor(start.saturating_sub(1))
                || caret == self.snap_floor(end.saturating_add(1))
        })
    }

    /// True when this block is an atomic chart/Mermaid fence. Those are the only
    /// blocks that keep their Markdown source visible while the caret is inside;
    /// every other block uses the Typora live-preview rendered projection.
    pub fn block_is_rich_fence(&self, idx: usize) -> bool {
        self.blocks.get(idx).is_some_and(|block| {
            block.spans.iter().any(|span| {
                matches!(
                    span.kind,
                    SpanKind::Object(ObjectKind::Diagram { .. })
                        | SpanKind::Object(ObjectKind::Chart { .. })
                )
            })
        })
    }

    // ------------------------------------------------------------ mapping

    /// Source offset -> render position. Total: it never fails, and the result
    /// is always a char boundary on both sides.
    pub fn src_to_render(&self, src: usize) -> RenderPos {
        let src = self.snap_floor(src);
        let Some(block_idx) = self.block_at_src(src) else {
            return RenderPos {
                block_idx: 0,
                inline_idx: 0,
                offset_in_inline: 0,
            };
        };
        let block = &self.blocks[block_idx];
        let (inline_idx, offset_in_inline) = locate(block, src);
        RenderPos {
            block_idx,
            inline_idx,
            offset_in_inline,
        }
    }

    /// Render position -> source offset. `None` when the position does not exist
    /// in this document; otherwise the offset is snapped down to a char boundary.
    pub fn render_to_src(&self, pos: &RenderPos) -> Option<usize> {
        let block = self.blocks.get(pos.block_idx)?;
        if block.spans.is_empty() {
            return (block.src.start == block.src.end
                && pos.inline_idx == 0
                && pos.offset_in_inline == 0)
                .then_some(block.src.start);
        }
        let span = block.spans.get(pos.inline_idx)?;
        if pos.offset_in_inline > span.src.len() {
            return None;
        }
        let raw = span.src.start + pos.offset_in_inline;
        Some(self.snap_floor(raw))
    }

    /// Visible projection: source offset -> byte offset inside `Block::render`.
    /// Zero-width markers collapse onto the render position they sit at, so this
    /// is many-to-one (a marker has no visible position of its own).
    pub fn src_to_render_offset(&self, src: usize) -> Option<(usize, usize)> {
        let src = self.snap_floor(src);
        let block_idx = self.block_at_src(src)?;
        let block = &self.blocks[block_idx];
        if block.spans.is_empty() {
            return Some((block_idx, 0));
        }
        let (i, off) = locate(block, src);
        let span = &block.spans[i];
        let render_byte = match span.kind {
            SpanKind::Text => span.render.start + off.min(span.render.len()),
            SpanKind::Marker(_) => span.render.start,
            SpanKind::Object(_) => span.render.start,
            SpanKind::Degraded => {
                let clamped = span.render.start + off.min(span.render.len());
                snap_str_floor(&block.render, clamped)
            }
        };
        Some((block_idx, render_byte.min(block.render.len())))
    }

    /// Rendered byte offset -> source offset (the canonical, smallest one: the
    /// marker run in front of a visible fragment wins over its content).
    pub fn render_offset_to_src(&self, block_idx: usize, render_byte: usize) -> Option<usize> {
        let block = self.blocks.get(block_idx)?;
        if render_byte > block.render.len() {
            return None;
        }
        let render_byte = snap_str_floor(&block.render, render_byte);
        let mut best: Option<usize> = None;
        for span in &block.spans {
            let candidate = match span.kind {
                SpanKind::Text => (span.render.start <= render_byte
                    && render_byte < span.render.end)
                    .then(|| span.src.start + (render_byte - span.render.start)),
                SpanKind::Degraded => {
                    let hit = span.render.start <= render_byte && render_byte <= span.render.end;
                    hit.then(|| {
                        let off = (render_byte - span.render.start).min(span.src.len());
                        self.snap_floor(span.src.start + off)
                    })
                }
                SpanKind::Marker(_) => (span.render.start == render_byte).then_some(span.src.start),
                // An object is atomic in the visible projection: every source
                // offset inside it collapses onto the placeholder's first byte.
                SpanKind::Object(_) => (span.render.start == render_byte).then_some(span.src.start),
            };
            if let Some(c) = candidate {
                best = Some(best.map_or(c, |b: usize| b.min(c)));
            }
        }
        Some(best.unwrap_or(if render_byte == 0 {
            block.src.start
        } else {
            block.src.end
        }))
    }
}

/// Locate `src` inside a block's span table. Spans tile the block, so a
/// position that is not strictly inside a span is the block end.
fn locate(block: &Block, src: usize) -> (usize, usize) {
    if block.spans.is_empty() {
        return (0, 0);
    }
    for (i, span) in block.spans.iter().enumerate() {
        if src >= span.src.start && src < span.src.end {
            return (i, src - span.src.start);
        }
    }
    let last = block.spans.len() - 1;
    (last, block.spans[last].src.len())
}

fn snap_str_floor(s: &str, byte: usize) -> usize {
    let mut byte = byte.min(s.len());
    while byte > 0 && !s.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

// ---------------------------------------------------------------- parsing

#[derive(Clone, Debug)]
struct Covered {
    src: Range<usize>,
    /// What the parser says this range renders as. Equal to the source slice for
    /// a clean span; different for escapes, entities, hard breaks, inline code.
    text: String,
    style: Style,
    /// Set for rich-content objects; `text` is then the U+FFFC placeholder.
    object: Option<ObjectKind>,
}

/// One fenced ```mermaid block being consumed. The whole fence becomes one
/// display object; a parse/render failure falls back to the source text.
#[derive(Clone, Debug)]
struct DiagramCapture {
    start: usize,
    source: String,
}

/// One image link being consumed. The parser reports the complete
/// `![alt](dest "title")` range on both `Start(Image)` and `End(Image)`, with the
/// alt text nested in between.
#[derive(Clone, Debug)]
struct ImageCapture {
    start: usize,
    destination: String,
    title: String,
    alt: String,
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    List,
    Block { idx: usize },
    Style { saved: Style },
}

#[derive(Debug)]
struct Builder {
    kind: BlockKind,
    covered: Vec<Covered>,
    fallback: Option<Range<usize>>,
    table: Option<TableData>,
}

struct Walk {
    builders: Vec<Builder>,
    frames: Vec<Frame>,
    style: Style,
    lists: Vec<bool>,
    /// Non-zero while inside `![alt](...)`: nested parser events are folded into
    /// one atomic image object instead of becoming spans of their own.
    image_depth: usize,
    image: Option<ImageCapture>,
    /// Current table while `BlockKind::Table` is open.
    table: Option<TableData>,
    cell_start: Option<usize>,
    /// Saved styles for the inline HTML tags currently open (`<b>`, `<i>`, ...).
    html_stack: Vec<Style>,
    /// Current ```mermaid fence, if one is open.
    diagram: Option<DiagramCapture>,
    /// Current ```chart fence, if one is open.
    chart: Option<DiagramCapture>,
}

impl Walk {
    fn current_block(&self) -> Option<usize> {
        self.frames.iter().rev().find_map(|f| match f {
            Frame::Block { idx } => Some(*idx),
            _ => None,
        })
    }

    fn open_block(&mut self, kind: BlockKind) -> usize {
        self.builders.push(Builder {
            kind,
            covered: Vec::new(),
            fallback: None,
            table: None,
        });
        let idx = self.builders.len() - 1;
        self.frames.push(Frame::Block { idx });
        idx
    }

    /// A paragraph inside a list item or a quote belongs to that block instead of
    /// opening one of its own.
    fn open_paragraph(&mut self) {
        let merge_into = self.current_block().filter(|&idx| {
            matches!(
                self.builders[idx].kind,
                BlockKind::ListItem { .. } | BlockKind::BlockQuote
            )
        });
        match merge_into {
            Some(idx) => self.frames.push(Frame::Block { idx }),
            None => {
                self.open_block(BlockKind::Paragraph);
            }
        }
    }

    fn push_style(&mut self, saved: Style, next: Style) {
        self.style = next;
        self.frames.push(Frame::Style { saved });
    }

    fn push_covered(&mut self, src: Range<usize>, text: String, style: Style) {
        if src.start >= src.end && text.is_empty() {
            return;
        }
        if let Some(idx) = self.current_block() {
            self.builders[idx].covered.push(Covered {
                src,
                text,
                style,
                object: None,
            });
        }
    }

    /// One atomic image/formula. `src` covers the complete Markdown syntax and
    /// `render` will be exactly one U+FFFC placeholder.
    fn push_object(&mut self, src: Range<usize>, kind: ObjectKind, style: Style) {
        if src.start >= src.end {
            return;
        }
        if let Some(idx) = self.current_block() {
            self.builders[idx].covered.push(Covered {
                src,
                text: "\u{FFFC}".to_string(),
                style,
                object: Some(kind),
            });
        }
    }

    /// Strip block-level HTML tags, decode the common entities, and map the
    /// visible text/styles back onto source ranges. This is a deliberately small
    /// subset: headings/bold/italic/code/links/lists/breaks, no CSS box layout.
    fn push_html_block(&mut self, range: Range<usize>, html: &str) {
        let mut out: Vec<(Range<usize>, String, Style)> = Vec::new();
        let mut style = self.style;
        let mut stack: Vec<Style> = Vec::new();
        let mut i = 0usize;
        let mut text_start = 0usize;
        let mut skip_depth = 0usize;
        let len = html.len();
        let flush = |out: &mut Vec<(Range<usize>, String, Style)>,
                     text_start: usize,
                     end: usize,
                     style: Style| {
            if text_start >= end {
                return;
            }
            let raw = &html[text_start..end];
            let decoded = decode_entities(raw);
            if !decoded.is_empty() {
                out.push((range.start + text_start..range.start + end, decoded, style));
            }
        };
        while i < len {
            if html.as_bytes()[i] == b'<' {
                if text_start < i && skip_depth == 0 {
                    flush(&mut out, text_start, i, style);
                }
                let Some(offset) = html[i..].find('>') else {
                    break;
                };
                let end = i + offset + 1;
                let tag = &html[i..end];
                let lower = tag.to_ascii_lowercase();
                let closing = lower.starts_with("</");
                if lower.starts_with("<script") || lower.starts_with("<style") {
                    skip_depth += 1;
                } else if closing && (lower.starts_with("</script") || lower.starts_with("</style"))
                {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if skip_depth == 0 {
                    if closing {
                        if let Some(previous) = stack.pop() {
                            style = previous;
                        }
                        if matches!(
                            lower.trim_end_matches('>').trim_start_matches("</"),
                            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                        ) {
                            out.push((range.start + i..range.start + end, "\n".to_string(), style));
                        }
                    } else if lower.starts_with("<br") {
                        out.push((range.start + i..range.start + end, "\n".to_string(), style));
                    } else if lower.starts_with("<b")
                        || lower.starts_with("<strong")
                        || lower.starts_with("<i")
                        || lower.starts_with("<em")
                        || lower.starts_with("<code")
                        || lower.starts_with("<kbd")
                        || lower.starts_with("<s>")
                        || lower.starts_with("<del")
                        || lower.starts_with("<a")
                        || lower.starts_with("<pre")
                    {
                        stack.push(style);
                        if lower.starts_with("<b") || lower.starts_with("<strong") {
                            style.bold = true;
                        } else if lower.starts_with("<i") || lower.starts_with("<em") {
                            style.italic = true;
                        } else if lower.starts_with("<code")
                            || lower.starts_with("<kbd")
                            || lower.starts_with("<pre")
                        {
                            style.code = true;
                        } else if lower.starts_with("<s>") || lower.starts_with("<del") {
                            style.strike = true;
                        } else {
                            style.link = true;
                        }
                    } else if lower.starts_with("<li") {
                        out.push((range.start + i..range.start + end, "• ".to_string(), style));
                    } else if matches!(
                        lower.trim_end_matches('>').trim_start_matches('<'),
                        "p" | "div" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    ) {
                        out.push((range.start + i..range.start + end, "\n".to_string(), style));
                    }
                }
                i = end;
                text_start = i;
                continue;
            }
            i += 1;
        }
        if text_start < len && skip_depth == 0 {
            flush(&mut out, text_start, len, style);
        }
        let first = out
            .iter()
            .position(|(_, content, _)| !content.trim().is_empty());
        let last = out
            .iter()
            .rposition(|(_, content, _)| !content.trim().is_empty());
        let out = match (first, last) {
            (Some(first), Some(last)) => &out[first..=last],
            _ => &out[..0],
        };
        let mut last_was_newline = false;
        for (src, content, style) in out {
            if content.trim().is_empty() && content != "\n" {
                continue;
            }
            if content == "\n" {
                if last_was_newline {
                    continue;
                }
                last_was_newline = true;
            } else {
                last_was_newline = false;
            }
            self.push_covered(src.clone(), content.clone(), *style);
        }
    }
}

fn decode_entities(raw: &str) -> String {
    raw.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// The small, safe inline-HTML subset typors renders instead of showing raw
/// tags. Unsupported tags are dropped, not echoed as text; the surrounding text
/// keeps its Markdown styling.
enum InlineHtml {
    Open(Style),
    Close,
    Break,
    Text,
}

fn inline_html_action(tag: &str, current: Style) -> InlineHtml {
    let lower = tag.trim().to_ascii_lowercase();
    match lower.as_str() {
        "<b>" | "<strong>" => InlineHtml::Open(Style {
            bold: true,
            ..current
        }),
        "<i>" | "<em>" => InlineHtml::Open(Style {
            italic: true,
            ..current
        }),
        "<code>" | "<kbd>" | "<samp>" => InlineHtml::Open(Style {
            code: true,
            ..current
        }),
        "<u>" => InlineHtml::Open(current),
        "<s>" | "<del>" | "<strike>" => InlineHtml::Open(Style {
            strike: true,
            ..current
        }),
        "<a>" => InlineHtml::Open(Style {
            link: true,
            ..current
        }),
        "<br>" | "<br/>" | "<br />" => InlineHtml::Break,
        _ if lower.starts_with("</") => InlineHtml::Close,
        // Unknown inline tags are stripped; their text remains.
        _ if lower.starts_with('<') => InlineHtml::Text,
        _ => InlineHtml::Text,
    }
}

fn parse_blocks(text: &str) -> Vec<Block> {
    let mut walk = Walk {
        builders: Vec::new(),
        frames: Vec::new(),
        style: Style::PLAIN,
        lists: Vec::new(),
        image_depth: 0,
        image: None,
        table: None,
        cell_start: None,
        html_stack: Vec::new(),
        diagram: None,
        chart: None,
    };

    for (event, range) in Parser::new_ext(
        text,
        Options::ENABLE_MATH | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    )
    .into_offset_iter()
    {
        // A fenced ```chart block is one display object.
        if let Some(chart) = &mut walk.chart {
            match event {
                Event::Text(s) => chart.source.push_str(&s),
                Event::End(TagEnd::CodeBlock) => {
                    let chart = walk.chart.take().expect("chart is open");
                    walk.push_object(
                        chart.start..range.end,
                        ObjectKind::Chart {
                            source: chart.source,
                        },
                        Style::PLAIN,
                    );
                    pop_frame(&mut walk.frames);
                }
                _ => {}
            }
            continue;
        }
        // A fenced ```mermaid block is one display object: consume the code text
        // and replace the whole fence with a placeholder on `End(CodeBlock)`.
        if let Some(diagram) = &mut walk.diagram {
            match event {
                Event::Text(s) => diagram.source.push_str(&s),
                Event::End(TagEnd::CodeBlock) => {
                    let diagram = walk.diagram.take().expect("diagram is open");
                    walk.push_object(
                        diagram.start..range.end,
                        ObjectKind::Diagram {
                            source: diagram.source,
                        },
                        Style::PLAIN,
                    );
                    pop_frame(&mut walk.frames);
                }
                _ => {}
            }
            continue;
        }
        // Inside `![alt](...)`, every nested event belongs to the image: the alt
        // text is accumulated, emphasis markers are dropped, and the whole thing
        // becomes one atomic object on `End(Image)`.
        if walk.image_depth > 0 {
            match event {
                Event::Text(s) => {
                    if let Some(image) = &mut walk.image {
                        image.alt.push_str(&s);
                    }
                }
                Event::End(TagEnd::Image) => {
                    walk.image_depth -= 1;
                    if walk.image_depth == 0
                        && let Some(image) = walk.image.take()
                    {
                        walk.push_object(
                            image.start..range.end,
                            ObjectKind::Image {
                                destination: image.destination,
                                title: image.title,
                                alt: image.alt,
                            },
                            walk.style,
                        );
                    }
                }
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::List(start) => {
                    walk.lists.push(start.is_some());
                    walk.frames.push(Frame::List);
                }
                Tag::Item => {
                    let ordered = *walk.lists.last().unwrap_or(&false);
                    let level = walk.lists.len().saturating_sub(1);
                    walk.open_block(BlockKind::ListItem { ordered, level });
                }
                Tag::Paragraph => walk.open_paragraph(),
                Tag::Heading { level, .. } => {
                    walk.open_block(BlockKind::Heading { level: level as u8 });
                }
                Tag::CodeBlock(kind) => {
                    let info = match &kind {
                        CodeBlockKind::Fenced(info) => info.trim().to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    if info.eq_ignore_ascii_case("mermaid") {
                        walk.open_block(BlockKind::Paragraph);
                        walk.diagram = Some(DiagramCapture {
                            start: range.start,
                            source: String::new(),
                        });
                    } else if info.eq_ignore_ascii_case("chart") {
                        walk.open_block(BlockKind::Paragraph);
                        walk.chart = Some(DiagramCapture {
                            start: range.start,
                            source: String::new(),
                        });
                    } else {
                        let kind = match kind {
                            CodeBlockKind::Fenced(info) => BlockKind::FencedCode {
                                info: info.to_string(),
                            },
                            CodeBlockKind::Indented => BlockKind::IndentedCode,
                        };
                        let idx = walk.open_block(kind);
                        walk.builders[idx].fallback = Some(range.clone());
                    }
                }
                Tag::BlockQuote(_) => {
                    walk.open_block(BlockKind::BlockQuote);
                }
                Tag::HtmlBlock => {
                    let idx = walk.open_block(BlockKind::Html);
                    walk.builders[idx].fallback = Some(range.clone());
                }
                Tag::Table(_) => {
                    walk.open_block(BlockKind::Table);
                    walk.table = Some(TableData { rows: Vec::new() });
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(table) = &mut walk.table {
                        table.rows.push(Vec::new());
                    }
                }
                Tag::TableCell => {
                    walk.cell_start = Some(range.start);
                }
                Tag::Emphasis => {
                    let saved = walk.style;
                    walk.push_style(
                        saved,
                        Style {
                            italic: true,
                            ..saved
                        },
                    );
                }
                Tag::Strong => {
                    let saved = walk.style;
                    walk.push_style(
                        saved,
                        Style {
                            bold: true,
                            ..saved
                        },
                    );
                }
                Tag::Strikethrough => {
                    let saved = walk.style;
                    walk.push_style(
                        saved,
                        Style {
                            strike: true,
                            ..saved
                        },
                    );
                }
                Tag::Link { .. } => {
                    let saved = walk.style;
                    walk.push_style(
                        saved,
                        Style {
                            link: true,
                            ..saved
                        },
                    );
                }
                Tag::Image {
                    dest_url, title, ..
                } => {
                    walk.image = Some(ImageCapture {
                        start: range.start,
                        destination: dest_url.to_string(),
                        title: title.to_string(),
                        alt: String::new(),
                    });
                    walk.image_depth = 1;
                }
                // Out-of-scope syntax: keep its text visible, no special block.
                _ => {
                    let saved = walk.style;
                    walk.push_style(saved, saved);
                }
            },
            Event::End(end) => match end {
                TagEnd::List(_) => {
                    walk.lists.pop();
                    pop_frame(&mut walk.frames);
                }
                TagEnd::Item
                | TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::BlockQuote(_)
                | TagEnd::HtmlBlock => pop_frame(&mut walk.frames),
                TagEnd::TableHead | TagEnd::TableRow => {}
                TagEnd::TableCell => {
                    if let Some(start) = walk.cell_start.take()
                        && let Some(table) = &mut walk.table
                        && let Some(row) = table.rows.last_mut()
                    {
                        row.push(start..range.end);
                    }
                }
                TagEnd::Table => {
                    if let Some(table) = walk.table.take()
                        && let Some(idx) = walk.current_block()
                    {
                        walk.builders[idx].table = Some(table);
                    }
                    pop_frame(&mut walk.frames);
                }
                TagEnd::Image => {}
                _ => {
                    if let Some(Frame::Style { saved }) = walk.frames.pop() {
                        walk.style = saved;
                    }
                }
            },
            Event::Text(s) => walk.push_covered(range, s.to_string(), walk.style),
            Event::Code(s) => {
                let slice = &text[range.clone()];
                let lead = slice.len() - slice.trim_start_matches('`').len();
                let trail = slice.len() - slice.trim_end_matches('`').len();
                if lead + trail < slice.len() {
                    walk.push_covered(
                        range.start + lead..range.end - trail,
                        s.to_string(),
                        Style {
                            code: true,
                            ..walk.style
                        },
                    );
                }
            }
            Event::Html(s) => {
                walk.push_html_block(range, &s);
            }
            Event::InlineHtml(s) => match inline_html_action(&s, walk.style) {
                InlineHtml::Open(style) => {
                    walk.html_stack.push(walk.style);
                    walk.style = style;
                }
                InlineHtml::Close => {
                    if let Some(saved) = walk.html_stack.pop() {
                        walk.style = saved;
                    }
                }
                InlineHtml::Break => {
                    walk.push_covered(range, "\n".to_string(), walk.style);
                }
                InlineHtml::Text => {
                    walk.push_covered(range, s.to_string(), walk.style);
                }
            },
            Event::InlineMath(s) => walk.push_object(
                range,
                ObjectKind::Math {
                    tex: s.to_string(),
                    display: false,
                },
                walk.style,
            ),
            Event::DisplayMath(s) => walk.push_object(
                range,
                ObjectKind::Math {
                    tex: s.to_string(),
                    display: true,
                },
                walk.style,
            ),
            Event::FootnoteReference(s) => {
                walk.push_covered(range, s.to_string(), walk.style);
            }
            Event::SoftBreak => walk.push_covered(range, "\n".to_string(), walk.style),
            Event::HardBreak => walk.push_covered(range, "\n".to_string(), walk.style),
            Event::Rule => {
                let idx = walk.open_block(BlockKind::Rule);
                walk.builders[idx].fallback = Some(range);
                pop_frame(&mut walk.frames);
            }
            // A task marker is drawn as a checkbox, never as text: leave it to the
            // gap scan so it becomes a zero-width marker.
            Event::TaskListMarker(_) => {}
        }
    }

    finalize(text, walk.builders)
}

fn pop_frame(frames: &mut Vec<Frame>) {
    frames.pop();
}

fn finalize(text: &str, builders: Vec<Builder>) -> Vec<Block> {
    struct Prelim {
        kind: BlockKind,
        src: Range<usize>,
        covered: Vec<Covered>,
        table: Option<TableData>,
    }

    let mut prelims: Vec<Prelim> = Vec::new();
    for builder in builders {
        let (first, last) = match (
            builder.covered.iter().map(|c| c.src.start).min(),
            builder.covered.iter().map(|c| c.src.end).max(),
        ) {
            (Some(first), Some(last)) => (first, last),
            _ => match builder.fallback.clone() {
                Some(fb) if fb.start < fb.end => (fb.start, fb.end),
                _ => continue,
            },
        };
        let mut start = line_start(text, first);
        let mut end = line_end(text, last);
        if matches!(builder.kind, BlockKind::FencedCode { .. }) {
            if start > 0 {
                let prev = line_start(text, start - 1);
                if is_fence_line(&text[prev..start]) {
                    start = prev;
                }
            }
            if end < text.len() {
                let next = line_end(text, end);
                if is_fence_line(&text[end..next]) {
                    end = next;
                }
            }
        }
        prelims.push(Prelim {
            kind: builder.kind,
            src: start..end,
            covered: builder.covered,
            table: builder.table,
        });
    }

    prelims.sort_by_key(|p| p.src.start);

    let mut blocks: Vec<Block> = Vec::new();
    let mut cursor = 0usize;
    for p in prelims {
        let start = p.src.start.max(cursor);
        let end = p.src.end.max(start);
        if start > cursor {
            blocks.push(gap_block(text, cursor..start));
        }
        if end > start {
            blocks.push(build_block(text, p.kind, start..end, &p.covered, p.table));
            cursor = end;
        }
    }
    if cursor < text.len() {
        blocks.push(gap_block(text, cursor..text.len()));
    }
    if blocks.is_empty() {
        // A blank document is still an editable document: one zero-length
        // paragraph gives the caret, hit testing and layout a block to own the
        // first line. `render_to_src` special-cases this block (it has no spans).
        blocks.push(Block {
            kind: BlockKind::Paragraph,
            src: 0..0,
            render: String::new(),
            spans: Vec::new(),
            table: None,
        });
    }
    expand_toc(text, &mut blocks);
    blocks
}

/// `[toc]` is expanded into a static outline of every heading. The block still
/// maps to its five source bytes, so editing the marker is a normal edit; the
/// generated lines are presentation only.
fn expand_toc(text: &str, blocks: &mut [Block]) {
    let Some(toc_idx) = blocks.iter().position(|block| {
        block.kind == BlockKind::Paragraph
            && text
                .get(block.src.clone())
                .is_some_and(|slice| slice.trim().eq_ignore_ascii_case("[toc]"))
    }) else {
        return;
    };
    let entries: Vec<(u8, String)> = blocks
        .iter()
        .filter_map(|block| match block.kind {
            BlockKind::Heading { level } => Some((level, block.render.clone())),
            _ => None,
        })
        .collect();
    let mut rendered = String::new();
    if entries.is_empty() {
        rendered.push_str("（暂无标题）");
    } else {
        for (index, (level, title)) in entries.iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&"  ".repeat((*level as usize).saturating_sub(1)));
            rendered.push_str("• ");
            rendered.push_str(title);
        }
    }
    let block = &mut blocks[toc_idx];
    block.kind = BlockKind::Paragraph;
    block.render = rendered.clone();
    block.spans = vec![Span {
        src: block.src.clone(),
        render: 0..rendered.len(),
        kind: SpanKind::Text,
        style: Style {
            link: true,
            ..Style::PLAIN
        },
    }];
}

fn build_block(
    text: &str,
    kind: BlockKind,
    src: Range<usize>,
    covered: &[Covered],
    table: Option<TableData>,
) -> Block {
    let mut candidates: Vec<&Covered> = covered
        .iter()
        .filter(|c| c.src.start < src.end && c.src.end > src.start)
        .collect();
    candidates.sort_by_key(|c| c.src.start);

    let mut spans: Vec<Span> = Vec::new();
    let mut render = String::new();
    let mut pos = src.start;

    for c in candidates {
        let cstart = c.src.start.max(pos);
        let cend = c.src.end.min(src.end);
        if cend <= cstart {
            continue;
        }
        if cstart > pos {
            push_marker(&mut spans, &mut render, text, pos..cstart, &kind);
        }
        let src_text = &text[cstart..cend];
        let rendered = if cstart == c.src.start && cend == c.src.end {
            c.text.clone()
        } else {
            src_text.to_string()
        };
        let span_kind = match &c.object {
            Some(object) => SpanKind::Object(object.clone()),
            None if rendered == src_text => SpanKind::Text,
            None => SpanKind::Degraded,
        };
        let rstart = render.len();
        render.push_str(&rendered);
        spans.push(Span {
            src: cstart..cend,
            render: rstart..render.len(),
            kind: span_kind,
            style: c.style,
        });
        pos = cend;
    }
    if pos < src.end {
        push_marker(&mut spans, &mut render, text, pos..src.end, &kind);
    }

    Block {
        kind,
        src,
        render,
        spans,
        table,
    }
}

fn push_marker(
    spans: &mut Vec<Span>,
    render: &mut String,
    text: &str,
    src: Range<usize>,
    block: &BlockKind,
) {
    if src.start >= src.end {
        return;
    }
    let kind = infer_marker(block, &text[src.clone()]);
    let at = render.len();
    spans.push(Span {
        src,
        render: at..at,
        kind: SpanKind::Marker(kind),
        style: Style::PLAIN,
    });
}

fn gap_block(text: &str, src: Range<usize>) -> Block {
    let rendered = text[src.clone()].to_string();
    let len = rendered.len();
    Block {
        kind: BlockKind::Gap,
        spans: vec![Span {
            src: src.clone(),
            render: 0..len,
            kind: SpanKind::Text,
            style: Style::PLAIN,
        }],
        render: rendered,
        src,
        table: None,
    }
}

fn infer_marker(block: &BlockKind, gap: &str) -> MarkerKind {
    let t = gap.trim_start_matches([' ', '\t']);
    if matches!(block, BlockKind::FencedCode { .. })
        && (t.starts_with("```") || t.starts_with("~~~"))
    {
        return MarkerKind::CodeFence;
    }
    if t.starts_with('#') {
        return MarkerKind::HeadingHash;
    }
    if t.starts_with('>') {
        return MarkerKind::QuoteMarker;
    }
    if t.starts_with("- [")
        || t.starts_with("* [")
        || t.starts_with("+ [")
        || t.starts_with("[ ]")
        || t.starts_with("[x]")
        || t.starts_with("[X]")
    {
        return MarkerKind::TaskBox;
    }
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return MarkerKind::ListBullet;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (t.contains(". ") || t.contains(") ")) {
        return MarkerKind::ListBullet;
    }
    if t.starts_with('`') || t.starts_with("~~~") {
        return MarkerKind::InlineCodeDelim;
    }
    if t.starts_with('*') || t.starts_with('_') {
        return MarkerKind::EmphasisDelim;
    }
    if t.contains('\n') && t.trim().is_empty() {
        return MarkerKind::LineBreak;
    }
    if t.starts_with('[')
        || t.starts_with(']')
        || t.starts_with('(')
        || t.starts_with(')')
        || t.starts_with('!')
    {
        return MarkerKind::LinkSyntax;
    }
    MarkerKind::Other
}

fn line_start(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut p = pos.min(text.len());
    while p > 0 && bytes[p - 1] != b'\n' {
        p -= 1;
    }
    p
}

/// Offset just past the newline of the line containing `pos`.
fn line_end(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut p = pos.min(text.len());
    while p < text.len() && bytes[p] != b'\n' {
        p += 1;
    }
    if p < text.len() {
        p += 1;
    }
    p
}

fn is_fence_line(line: &str) -> bool {
    let t = line.trim_start_matches([' ', '\t']);
    t.starts_with("```") || t.starts_with("~~~")
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    fn blocks(text: &str) -> Vec<Block> {
        Document::parse(text).blocks().to_vec()
    }

    /// 2.1 — six block categories: block count and source ranges.
    #[test]
    fn block_count_and_ranges_for_six_categories() {
        // heading
        let text = "## 二级标题\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 1, "heading: {bs:?}");
        assert_eq!(bs[0].kind, BlockKind::Heading { level: 2 });
        assert_eq!(bs[0].src, 0..text.len());
        assert_eq!(bs[0].render, "二级标题");

        // bold (inline)
        let text = "这是**粗体**文本\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 1, "bold: {bs:?}");
        assert_eq!(bs[0].kind, BlockKind::Paragraph);
        assert_eq!(bs[0].src, 0..text.len());
        assert_eq!(bs[0].render, "这是粗体文本");
        let bold: Vec<&Span> = bs[0].spans.iter().filter(|s| s.style.bold).collect();
        assert_eq!(bold.len(), 1, "bold spans: {:?}", bs[0].spans);
        assert_eq!(&text[bold[0].src.clone()], "粗体");

        // italic (inline)
        let text = "这是*斜体*文本\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 1, "italic: {bs:?}");
        assert_eq!(bs[0].render, "这是斜体文本");
        let italic: Vec<&Span> = bs[0].spans.iter().filter(|s| s.style.italic).collect();
        assert_eq!(italic.len(), 1, "italic spans: {:?}", bs[0].spans);
        assert_eq!(&text[italic[0].src.clone()], "斜体");

        // inline code
        let text = "使用`行内代码`渲染\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 1, "inline code: {bs:?}");
        assert_eq!(bs[0].render, "使用行内代码渲染");
        let code: Vec<&Span> = bs[0].spans.iter().filter(|s| s.style.code).collect();
        assert_eq!(code.len(), 1, "code spans: {:?}", bs[0].spans);
        assert_eq!(&text[code[0].src.clone()], "行内代码");

        // fenced code block
        let text = "```rust\nfn main() {}\n```\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 1, "fence: {bs:?}");
        assert_eq!(
            bs[0].kind,
            BlockKind::FencedCode {
                info: "rust".to_string()
            }
        );
        assert_eq!(bs[0].src, 0..text.len());
        assert_eq!(bs[0].render, "fn main() {}\n");

        // list
        let first_line = "- 第一项\n";
        let text = "- 第一项\n- 第二项\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 2, "list: {bs:?}");
        assert!(bs.iter().all(|b| b.kind
            == BlockKind::ListItem {
                ordered: false,
                level: 0
            }));
        assert_eq!(bs[0].src, 0..first_line.len());
        assert_eq!(bs[1].src, first_line.len()..text.len());
        assert_eq!(bs[0].render, "第一项");
        assert_eq!(bs[1].render, "第二项");

        // ordered list keeps its ordinal out of the rendered text
        let text = "1. 一\n2. 二\n";
        let bs = blocks(text);
        assert_eq!(bs.len(), 2, "ordered list: {bs:?}");
        assert!(bs.iter().all(|b| b.kind
            == BlockKind::ListItem {
                ordered: true,
                level: 0
            }));
        assert_eq!(bs[1].render, "二");
    }

    /// 2.1 / D3 — blocks tile the document and spans tile each block, and the
    /// rendered text is exactly the concatenation of the visible spans.
    #[test]
    fn blocks_and_spans_tile_without_gaps() {
        let mut samples = sample::mixed_samples(100);
        samples.push(String::new());
        samples.push("\n\n\n".to_string());

        for text in &samples {
            let doc = Document::parse(text);
            assert_eq!(doc.len_bytes(), text.len());

            let mut cursor = 0;
            for block in doc.blocks() {
                assert_eq!(block.src.start, cursor, "block gap in {text:?}");
                cursor = block.src.end;
                if block.src.start == block.src.end {
                    // Only the synthetic zero-length paragraph of an empty
                    // document is allowed to be empty.
                    assert_eq!(text, "");
                    assert_eq!(doc.blocks().len(), 1);
                    assert!(block.spans.is_empty());
                    continue;
                }
                assert!(block.src.start < block.src.end);

                let mut span_cursor = block.src.start;
                let mut render_cursor = 0;
                let mut render = String::new();
                for span in &block.spans {
                    assert_eq!(span.src.start, span_cursor, "span gap in {text:?}");
                    assert!(span.src.start < span.src.end, "empty span in {text:?}");
                    span_cursor = span.src.end;
                    assert_eq!(span.render.start, render_cursor, "render gap in {text:?}");
                    render_cursor = span.render.end;
                    render.push_str(&block.render[span.render.clone()]);
                    match span.kind {
                        SpanKind::Text => {
                            assert_eq!(&text[span.src.clone()], &block.render[span.render.clone()])
                        }
                        SpanKind::Marker(_) => assert_eq!(span.render.start, span.render.end),
                        SpanKind::Degraded => {}
                        SpanKind::Object(_) => {
                            assert_eq!(&block.render[span.render.clone()], "\u{FFFC}");
                            assert_eq!(span.render.len(), '\u{FFFC}'.len_utf8());
                        }
                    }
                }
                assert_eq!(span_cursor, block.src.end, "spans do not tile {text:?}");
                assert_eq!(render_cursor, block.render.len());
                assert_eq!(render, block.render);
            }
            assert_eq!(
                cursor,
                text.len(),
                "blocks do not reach the end of {text:?}"
            );
        }
    }

    /// 2.2 — every offset of 100 mixed samples round-trips source -> render ->
    /// source. Interior bytes of a character snap down, which is asserted too.
    #[test]
    fn src_to_render_roundtrip_is_exact_for_every_offset() {
        let samples = sample::mixed_samples(100);
        assert_eq!(samples.len(), 100);

        let mut checked = 0usize;
        for (n, text) in samples.iter().enumerate() {
            let doc = Document::parse(text);
            checked += doc.len_bytes() + 1;
            for s in 0..=doc.len_bytes() {
                let pos = doc.src_to_render(s);
                let back = doc
                    .render_to_src(&pos)
                    .unwrap_or_else(|| panic!("sample {n} offset {s}: no reverse mapping"));
                let expected = if doc.is_char_boundary(s) {
                    s
                } else {
                    doc.snap_floor(s)
                };
                assert_eq!(back, expected, "sample {n} ({text:?}) offset {s}");
                assert!(
                    doc.is_char_boundary(back),
                    "sample {n} offset {s} mapped to a non-boundary"
                );
                let block = &doc.blocks()[pos.block_idx];
                assert!(
                    block.src.start <= expected && expected <= block.src.end,
                    "sample {n} offset {s} left its block"
                );
                let span = &block.spans[pos.inline_idx];
                assert_eq!(span.src.start + pos.offset_in_inline, expected);
            }
        }
        println!(
            "roundtrip checked {checked} source offsets across {} samples",
            samples.len()
        );
        assert!(
            checked > 20_000,
            "sample corpus is too small: {checked} offsets"
        );
    }

    /// 2.2 — the reverse of the visible projection is the canonical smallest
    /// source offset, per rendered byte, on every sample.
    #[test]
    fn render_offset_to_src_is_canonical() {
        use std::collections::HashMap;

        let mut samples = sample::mixed_samples(100);
        samples.push("**a**b*\n".to_string());
        samples.push("[](x)\n".to_string());

        for (n, text) in samples.iter().enumerate() {
            let doc = Document::parse(text);
            let mut canonical: HashMap<(usize, usize), usize> = HashMap::new();
            for s in 0..=doc.len_bytes() {
                if !doc.is_char_boundary(s) {
                    continue;
                }
                if let Some((b, r)) = doc.src_to_render_offset(s) {
                    canonical
                        .entry((b, r))
                        .and_modify(|v| *v = (*v).min(s))
                        .or_insert(s);
                }
            }
            for ((b, r), want) in canonical {
                let got = doc.render_offset_to_src(b, r);
                assert_eq!(got, Some(want), "sample {n} ({text:?}) block {b} byte {r}");
            }
        }
    }

    /// 3.2 - math events become objects, including the delimiters in `src` but
    /// not in `tex`; currency text is not mistaken for math.
    #[test]
    fn math_objects_keep_tex_and_cover_delimiters() {
        let text = "行内 $E = mc^2$ 与块级 $$\n\\frac{a}{b}\n$$\n";
        let doc = Document::parse(text);
        let objects: Vec<_> = doc
            .blocks()
            .iter()
            .flat_map(|b| &b.spans)
            .filter_map(|s| match &s.kind {
                SpanKind::Object(kind) => Some((s.src.clone(), kind.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(objects.len(), 2, "{objects:?}");
        assert_eq!(&text[objects[0].0.clone()], "$E = mc^2$");
        assert!(matches!(
            &objects[0].1,
            ObjectKind::Math { tex, display: false } if tex == "E = mc^2"
        ));
        assert_eq!(&text[objects[1].0.clone()], "$$\n\\frac{a}{b}\n$$");
        assert!(matches!(
            &objects[1].1,
            ObjectKind::Math { tex, display: true } if tex.contains("\\frac{a}{b}")
        ));

        let money = Document::parse("价格 $5 和 $6\n");
        assert!(
            money
                .blocks()
                .iter()
                .flat_map(|b| &b.spans)
                .all(|s| !matches!(s.kind, SpanKind::Object(_)))
        );
        let escaped = Document::parse("\\$not math\\$\n");
        assert!(
            escaped
                .blocks()
                .iter()
                .flat_map(|b| &b.spans)
                .all(|s| !matches!(s.kind, SpanKind::Object(_)))
        );
    }

    /// Block HTML is stripped to visible text and inline styles; tags themselves
    /// must not appear in the render projection.
    #[test]
    fn html_blocks_strip_tags_and_keep_text() {
        let text = "<div><h2>标题</h2><p>正文 <b>粗体</b> 与 <i>斜体</i><br>下一行</p></div>\n";
        let doc = Document::parse(text);
        let rendered: String = doc
            .blocks()
            .iter()
            .map(|block| block.render.as_str())
            .collect();
        assert!(rendered.contains("标题"), "{rendered:?}");
        assert!(rendered.contains("粗体"), "{rendered:?}");
        assert!(rendered.contains("斜体"), "{rendered:?}");
        assert!(!rendered.contains('<'), "tags survived: {rendered:?}");
        for s in 0..=doc.len_bytes() {
            let pos = doc.src_to_render(s);
            assert_eq!(
                doc.render_to_src(&pos),
                Some(doc.snap_floor(s)),
                "offset {s}"
            );
        }
    }

    /// 3.3 - one image syntax becomes exactly one object, with alt/title/dest    /// captured and the inner Markdown markers folded away.
    #[test]
    fn images_become_one_atomic_object() {
        let text = "前 ![alt *em*](<my image.png> \"标题\") 后\n";
        let doc = Document::parse(text);
        let objects: Vec<_> = doc
            .blocks()
            .iter()
            .flat_map(|b| &b.spans)
            .filter_map(|s| match &s.kind {
                SpanKind::Object(ObjectKind::Image {
                    destination,
                    title,
                    alt,
                }) => Some((
                    s.src.clone(),
                    destination.clone(),
                    title.clone(),
                    alt.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(objects.len(), 1, "{objects:?}");
        assert_eq!(
            &text[objects[0].0.clone()],
            "![alt *em*](<my image.png> \"标题\")"
        );
        assert_eq!(objects[0].1, "my image.png");
        assert_eq!(objects[0].2, "标题");
        assert_eq!(objects[0].3, "alt em");

        let linked = Document::parse("[![inner](i.png)](https://x)\n");
        let count = linked
            .blocks()
            .iter()
            .flat_map(|b| &b.spans)
            .filter(|s| matches!(s.kind, SpanKind::Object(ObjectKind::Image { .. })))
            .count();
        assert_eq!(count, 1);
    }

    /// 3.1 - object-bearing documents keep the lossless mapping and collapse
    /// source offsets inside an object onto the placeholder in the visible one.
    #[test]
    fn object_offsets_roundtrip_and_collapse_in_the_visible_projection() {
        let text = "行内 $x^2$ 与 ![图](a.png) 并排\n\n$$\\frac{a}{b}$$\n";
        let doc = Document::parse(text);
        let mut object_spans = 0usize;
        for s in 0..=doc.len_bytes() {
            let expected = doc.snap_floor(s);
            let pos = doc.src_to_render(s);
            assert_eq!(doc.render_to_src(&pos), Some(expected), "offset {s}");
            let block = &doc.blocks()[pos.block_idx];
            let span = &block.spans[pos.inline_idx];
            if let SpanKind::Object(_) = span.kind {
                object_spans += 1;
                let (b, r) = doc.src_to_render_offset(s).expect("visible projection");
                assert_eq!(b, pos.block_idx);
                assert_eq!(&block.render[r..r + '\u{FFFC}'.len_utf8()], "\u{FFFC}");
            }
        }
        assert!(object_spans >= 3, "expected objects to be exercised");
    }

    /// 2.3 — UTF-8 boundary invariant on CJK, composed emoji and nested emphasis.
    #[test]
    fn utf8_boundaries_hold_on_cjk_emoji_and_nested_emphasis() {
        let samples = [
            // CJK
            "# 中文标题\n正文含中日韩字符，标点也全角。\n",
            // composed emoji: ZWJ family, skin tone, combining accent, flags
            "家庭 👨‍👩‍👧‍👦 与肤色 👍🏽 与 é (e + U+0301) 与 🇨🇳\n",
            // nested emphasis: italic inside bold
            "外层**粗体里嵌 *斜体 中文* 与外层 😀**结尾\n",
            // inline code with multi-byte content plus a fenced block
            "看 `代码 🀄` 与\n\n```rust\nfn 名() { \"字\" }\n```\n",
            // ordered list with an indented item
            "1. 第一项 😀\n   - 缩进项 漢字\n",
        ];

        for text in samples {
            let doc = Document::parse(text);
            let mut saw_multibyte = false;

            for s in 0..=doc.len_bytes() {
                let pos = doc.src_to_render(s);

                if doc.is_char_boundary(s) {
                    let block = &doc.blocks()[pos.block_idx];
                    let span = &block.spans[pos.inline_idx];
                    let span_src = &text[span.src.clone()];
                    assert!(
                        span_src.is_char_boundary(pos.offset_in_inline),
                        "offset inside span is not a char boundary: {text:?} @ {s}"
                    );
                    // both sides of the position must slice cleanly, i.e. no half
                    // character and no lone surrogate can appear
                    let head = &text[..s];
                    let tail = &text[s..];
                    assert_eq!(format!("{head}{tail}"), text);

                    match span.kind {
                        SpanKind::Text => {
                            let render_byte = span.render.start + pos.offset_in_inline;
                            assert!(
                                block.render.is_char_boundary(render_byte),
                                "visible span offset is not a render char boundary"
                            );
                            assert_eq!(
                                &block.render[span.render.start..render_byte],
                                &span_src[..pos.offset_in_inline],
                                "visible span offset differs between the two sides"
                            );
                        }
                        SpanKind::Marker(_) => assert_eq!(span.render.start, span.render.end),
                        SpanKind::Degraded => {
                            assert!(pos.offset_in_inline <= span.render.len());
                        }
                        SpanKind::Object(_) => {
                            assert_eq!(&block.render[span.render.clone()], "\u{FFFC}");
                            assert!(pos.offset_in_inline <= span.src.len());
                        }
                    }

                    if let Some((b, r)) = doc.src_to_render_offset(s) {
                        let render = &doc.blocks()[b].render;
                        assert!(
                            render.is_char_boundary(r),
                            "render offset is not a char boundary: {text:?} @ {s}"
                        );
                    }
                    if s < text.len() && !text.is_char_boundary(s + 1) {
                        saw_multibyte = true;
                    }
                } else {
                    // a byte inside a character must never become a position of
                    // its own: it snaps to the character start
                    let back = doc.render_to_src(&pos).unwrap();
                    assert!(text.is_char_boundary(back));
                    assert!(back <= s);
                }
            }
            assert!(
                saw_multibyte || text.is_ascii(),
                "sample was expected to carry multi-byte characters: {text:?}"
            );
        }
    }

    /// Out-of-scope syntax (table, math, image, raw HTML) must degrade without
    /// panic and keep the mapping total.
    #[test]
    fn out_of_scope_syntax_degrades_without_panic() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n\n<script>x</script>\n\n脚注[^1]\n";
        let doc = Document::parse(text);
        assert!(!doc.blocks().is_empty());
        for s in 0..=doc.len_bytes() {
            let pos = doc.src_to_render(s);
            assert_eq!(doc.render_to_src(&pos), Some(doc.snap_floor(s)));
        }
    }

    /// Editing the single buffer reparses and keeps the invariants.
    #[test]
    fn edit_reparses_and_keeps_mapping() {
        let mut doc = Document::parse("# 标题\n\n正文**粗体**\n");
        let before = doc.len_bytes();
        doc.apply_edit(0..0, "> ");
        assert_eq!(doc.len_bytes(), before + 2);
        assert_eq!(doc.revision(), 1);
        assert_eq!(doc.text(), "> # 标题\n\n正文**粗体**\n");
        for s in 0..=doc.len_bytes() {
            let pos = doc.src_to_render(s);
            assert_eq!(doc.render_to_src(&pos), Some(doc.snap_floor(s)));
        }
        doc.apply_edit(0..2, "");
        assert_eq!(doc.text(), "# 标题\n\n正文**粗体**\n");
    }

    /// A blank document is still editable: one zero-length paragraph owns offset
    /// 0, and the mapping round-trips through that block.
    #[test]
    fn empty_document_has_one_editable_block() {
        let doc = Document::parse("");
        assert_eq!(doc.blocks().len(), 1);
        assert_eq!(doc.blocks()[0].src, 0..0);
        assert_eq!(doc.block_at_src(0), Some(0));
        let pos = doc.src_to_render(0);
        assert_eq!(pos.block_idx, 0);
        assert_eq!(pos.inline_idx, 0);
        assert_eq!(pos.offset_in_inline, 0);
        assert_eq!(doc.render_to_src(&pos), Some(0));
        assert_eq!(doc.text(), "");
    }

    #[test]
    fn empty_document_roundtrips_through_edit() {
        let mut doc = Document::parse("");
        doc.apply_edit(0..0, "x");
        assert_eq!(doc.text(), "x");
        assert!(!doc.blocks().is_empty());
        doc.apply_edit(0..1, "");
        assert_eq!(doc.text(), "");
        assert_eq!(doc.blocks().len(), 1);
        assert_eq!(doc.blocks()[0].src, 0..0);
        assert_eq!(doc.block_at_src(0), Some(0));
    }

    #[test]
    fn toc_marker_expands_to_heading_outline() {
        let doc = Document::parse("[toc]\n\n# 标题一\n\n## 标题二\n");
        let toc = doc
            .blocks()
            .iter()
            .find(|block| block.render.contains("标题一"))
            .expect("toc block");
        assert!(toc.render.contains("• 标题一"));
        assert!(toc.render.contains("  • 标题二"));
        assert!(toc.spans.iter().any(|span| span.style.link));
    }

    /// Degenerate documents must not panic.
    #[test]
    fn empty_documents_are_safe() {
        for text in ["", "\n", "\n\n", "   ", "#", "**", "```"] {
            let doc = Document::parse(text);
            let _ = doc.src_to_render(0);
            let _ = doc.src_to_render(doc.len_bytes());
            for s in 0..=doc.len_bytes() {
                let pos = doc.src_to_render(s);
                let _ = doc.render_to_src(&pos);
                let _ = doc.src_to_render_offset(s);
            }
            for b in 0..doc.blocks().len() {
                let _ = doc.render_offset_to_src(b, 0);
            }
        }
    }

    /// 已知的解析器偏差（交 M1）：`pulldown-cmark 0.13.5` 判定分隔符 run 的
    /// right-flanking 时，把紧邻闭合 `**` 的 emoji（Unicode `So` 类）当成标点，
    /// 于是外层 strong 不成立、`**` 作为普通文本留在 `block.render` 里。
    /// CommonMark 的标点定义是 `P*` 类，`So` 不在其中，所以这是库级偏差，
    /// 不是 H3 映射的缺陷（本测试同时证明往返与内层 Marker 都正常）。
    ///
    /// 注意：下面第一条断言钉住的是**当前已知的错误行为**，不是正确行为。
    /// M1 换解析器 / 给 pulldown-cmark 打补丁 / 自写 flanking 判定之后，
    /// 这条测试应当失败——届时把断言改成修正后的期望，别把它当回归拦住修复。
    #[test]
    fn known_deviation_emoji_after_closing_strong_keeps_markers() {
        // 偏差样本：闭合 `**` 的左邻是 emoji。
        let text = "外层**粗体里嵌 *斜体* 与 emoji 😀**结尾。";
        let doc = Document::parse(text);
        assert_eq!(doc.blocks().len(), 1, "偏差样本不应被拆成多块");
        let block = &doc.blocks()[0];
        println!("deviation render = {:?}", block.render);

        assert!(
            block.render.contains("**"),
            "偏差消失了（好事）：pulldown-cmark 这里不再残留 `**`，请把本测试改成修正后的期望。render = {:?}",
            block.render
        );

        // 偏差不是 panic、也不是映射缺陷：每个偏移的往返仍然成立。
        for s in 0..=doc.len_bytes() {
            let pos = doc.src_to_render(s);
            assert_eq!(
                doc.render_to_src(&pos),
                Some(doc.snap_floor(s)),
                "偏差样本在偏移 {s} 上往返失败"
            );
        }

        // 内层 `*斜体*` 的分隔符仍然是被渲染器忽略的零宽 Marker。
        let zero_width: Vec<&Span> = block
            .spans
            .iter()
            .filter(|span| {
                matches!(span.kind, SpanKind::Marker(MarkerKind::EmphasisDelim))
                    && span.render.start == span.render.end
            })
            .collect();
        assert!(
            zero_width.len() >= 2,
            "内层强调的标记应保持零宽：{:?}",
            block.spans
        );

        // 规避写法：把 emoji 与闭合 `**` 拆开（中间隔一个空格与正文），外层 strong
        // 立刻成立、标记全部零宽。这段文本就是 sample.md 第 23 行的当前内容。
        let spelled = "嵌套强调：外层**粗体里嵌 *斜体中文* 与外层 emoji 😀 的粗体**结尾。";
        let spelled_doc = Document::parse(spelled);
        let spelled_block = &spelled_doc.blocks()[0];
        println!("workaround render = {:?}", spelled_block.render);
        assert!(
            !spelled_block.render.contains('*'),
            "规避写法仍残留标记：{:?}",
            spelled_block.render
        );
        assert!(
            spelled_block.spans.iter().any(|span| span.style.italic),
            "规避写法丢了内层斜体：{:?}",
            spelled_block.spans
        );
    }
}
