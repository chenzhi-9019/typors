//! The GPUI side of the render projection: focus, caret, the block projection
//! switch, and the one function through which text changes.
//!
//! What lives here and what does not:
//!
//! * Layout, which fragments are visible, and byte -> x mapping: `render.rs`.
//! * This file: `request_layout` (measures through the window text system and
//!   hands gpui the total height to Taffy), `prepaint` (shapes every fragment
//!   once), `paint` (quads, text, caret), and the editor state a caret needs.
//!
//! The layering that makes group 4 testable without a window:
//!
//! * The caret is a *source byte offset* ([`MarkdownView::caret`]) and nothing
//!   else. Block index, display offset and screen rectangle are all derived, so
//!   a re-parse cannot leave the caret pointing at a stale block.
//! * Every projection decision is [`LayoutMode`] — at most one block is source —
//!   and every caret/geometry query round-trips through [`DocLayout`], which
//!   works with any [`typors_render::Metrics`] (window text system or
//!   [`typors_render::ApproxMetrics`]).
//! * [`MarkdownView::apply_edit`] is the only function that mutates the buffer.
//!
//! Group 6 (IME) plugs into exactly two places, both already designed for it:
//! `paint` calls `window.handle_input` (that is the only legal place for it,
//! `window.rs:5391`), and [`MarkdownView`]'s `EntityInputHandler` impl routes the
//! commit into [`MarkdownView::apply_edit`].

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::gpui::*;

use crate::capture::{
    DumpCapture, DumpSink, ImeSink, MediaProbeFn, MediaSink, PerfProbeFn, PerfSink, SelectSink,
};
use crate::ime::MarkedText;
use crate::media::{MediaContext, paint_image};
use crate::selection::{HighlightRect, History, HistoryAction, Selection, undo_range};
use typors_core::doc::BlockKind;
use typors_core::Document;
use typors_render::{
    BlockCache, DocLayout, DrawSpan, EditorTheme, LayoutMode, Metrics, ShapeCache, Viewport,
    WindowMetrics,
};

/// Slack kept above and below the visible band when culling, as a fraction of the
/// viewport height (task 7.1). One full viewport: a fragment that straddles the
/// edge, a sub-pixel scroll step, and a window that resizes within a frame all
/// stay inside it, while a 100k-character document still drops ~97% of its
/// fragments.
pub const CULL_MARGIN: f32 = 1.0;

/// `TYPORS_SCROLL=<px>`: scroll the page to that document offset on the first
/// frame, and `TYPORS_CULL_LOG=1`: print the culling band and the shaped/culled
/// fragment counts once per frame.
///
/// Both exist because the agent that wrote task 7.1 cannot see a window or scroll
/// one: the culling band is derived from the element's *scrolled* bounds
/// (`-bounds.origin.y`), and that derivation is only trustworthy if it can be
/// observed on a genuinely scrolled window instead of a synthetic number. With
/// these two, `TYPORS_SCROLL=120000 TYPORS_CULL_LOG=1 cargo run --release --
/// sample.md` prints the band the real scroll produced.
fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("on") | Ok("true")
    )
}

/// Which entry point changed the buffer.
///
/// Every one of them funnels into [`MarkdownView::apply_edit_internal`], so the
/// tag only exists to make the `TYPORS_EDIT_LOG` trace say *who* typed — the
/// distinction that decides "one newline per Enter" from "the Enter key and a
/// `\r` both arrived". It never changes what the edit does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTrigger {
    /// The `Newline` action (`enter` binding).
    Newline,
    /// A character from the platform: `WM_CHAR` → `replace_text_in_range`.
    Char,
    /// IME commit: the composition result landing in the buffer.
    ImeCommit,
    Backspace,
    Delete,
    Paste,
    Cut,
    Undo,
    Redo,
    /// An edit made through the public [`MarkdownView::apply_edit`] API.
    Api,
    /// A menu formatting command (bold/heading/list/...).
    Format,
}

impl EditTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            EditTrigger::Newline => "newline",
            EditTrigger::Char => "char",
            EditTrigger::ImeCommit => "ime-commit",
            EditTrigger::Backspace => "backspace",
            EditTrigger::Delete => "delete",
            EditTrigger::Paste => "paste",
            EditTrigger::Cut => "cut",
            EditTrigger::Undo => "undo",
            EditTrigger::Redo => "redo",
            EditTrigger::Api => "api",
            EditTrigger::Format => "format",
        }
    }
}

/// A command from the menu that changes Markdown formatting at the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    Heading(u8),
    Paragraph,
    BulletList,
    OrderedList,
    Quote,
    CodeBlock,
    Bold,
    Italic,
    Strike,
    InlineCode,
    InlineMath,
    Link,
    Image,
}

impl MarkdownView {
    /// Apply one formatting command through the normal edit/undo path.
    pub fn apply_editor_command(&mut self, command: EditorCommand) -> bool {
        let before = self.doc.revision();
        match command {
            EditorCommand::Bold => self.wrap_selection("**", "**"),
            EditorCommand::Italic => self.wrap_selection("*", "*"),
            EditorCommand::Strike => self.wrap_selection("~~", "~~"),
            EditorCommand::InlineCode => self.wrap_selection("`", "`"),
            EditorCommand::InlineMath => self.wrap_selection("$", "$"),
            EditorCommand::Link => self.wrap_selection("[", "](https://)"),
            EditorCommand::Image => self.wrap_selection("![", "](assets/image.png)"),
            EditorCommand::Heading(level) => {
                self.set_line_prefix(&format!("{} ", "#".repeat(level as usize)))
            }
            EditorCommand::Paragraph => self.set_line_prefix(""),
            EditorCommand::BulletList => self.toggle_line_prefix("- ", &["- ", "* ", "+ "]),
            EditorCommand::OrderedList => self.toggle_line_prefix("1. ", &["1. "]),
            EditorCommand::Quote => self.toggle_line_prefix("> ", &["> "]),
            EditorCommand::CodeBlock => self.wrap_lines("```\n", "\n```"),
        }
        self.doc.revision() != before
    }

    fn wrap_selection(&mut self, prefix: &str, suffix: &str) {
        if let Some(range) = self.selection_range() {
            let text = self.doc.text()[range.clone()].to_string();
            let inserted = format!("{prefix}{text}{suffix}");
            let start = range.start;
            self.apply_edit_internal(range, &inserted, HistoryAction::Record, EditTrigger::Format);
            self.anchor = Some(start + prefix.len());
            self.caret = Some(start + prefix.len() + text.len());
            return;
        }
        let at = self.caret.unwrap_or(0);
        let inserted = format!("{prefix}{suffix}");
        self.apply_edit_internal(
            at..at,
            &inserted,
            HistoryAction::Record,
            EditTrigger::Format,
        );
        self.caret = Some(at + prefix.len());
        self.anchor = self.caret;
    }

    fn wrap_lines(&mut self, prefix: &str, suffix: &str) {
        let (start, end) = self
            .selection_range()
            .map_or((self.caret.unwrap_or(0), self.caret.unwrap_or(0)), |r| {
                (r.start, r.end)
            });
        let text = self.doc.text()[start..end].to_string();
        let inserted = format!("{prefix}{text}{suffix}");
        self.apply_edit_internal(
            start..end,
            &inserted,
            HistoryAction::Record,
            EditTrigger::Format,
        );
        self.anchor = Some(start + prefix.len());
        self.caret = Some(start + prefix.len() + text.len());
    }

    fn set_line_prefix(&mut self, prefix: &str) {
        let (line_start, line_end) = self.current_line();
        let text = self.doc.text();
        let line = &text[line_start..line_end];
        let (existing, content) = strip_line_prefix(line);
        let _ = existing;
        let new_line = format!("{prefix}{content}");
        self.replace_line(line_start, line_end, &new_line);
    }

    fn toggle_line_prefix(&mut self, prefix: &str, known: &[&str]) {
        let (line_start, line_end) = self.current_line();
        let text = self.doc.text();
        let line = &text[line_start..line_end];
        let (existing, content) = strip_line_prefix(line);
        let new_line = if known.contains(&existing) {
            content.to_string()
        } else {
            format!("{prefix}{content}")
        };
        self.replace_line(line_start, line_end, &new_line);
    }

    fn replace_line(&mut self, line_start: usize, line_end: usize, new_line: &str) {
        let caret = self.caret.unwrap_or(line_start);
        let old_prefix = caret.saturating_sub(line_start).min(new_line.len());
        self.apply_edit_internal(
            line_start..line_end,
            new_line,
            HistoryAction::Record,
            EditTrigger::Format,
        );
        self.caret = Some((line_start + old_prefix).min(line_start + new_line.len()));
        self.anchor = self.caret;
    }

    fn current_line(&self) -> (usize, usize) {
        let caret = self.caret.unwrap_or(0);
        let text = self.doc.text();
        let start = text[..caret].rfind('\n').map_or(0, |index| index + 1);
        let end = text[caret..]
            .find('\n')
            .map_or(text.len(), |index| caret + index);
        (start, end)
    }
}

fn strip_line_prefix(line: &str) -> (&str, &str) {
    if let Some(index) = line.find(". ")
        && line[..index].chars().all(|ch| ch.is_ascii_digit())
        && index > 0
    {
        return (&line[..index + 2], &line[index + 2..]);
    }
    for prefix in ["- ", "* ", "+ ", "> ", "- [ ] ", "- [x] "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return (prefix, rest);
        }
    }
    if let Some(rest) = line.strip_prefix("# ") {
        return ("# ", rest);
    }
    for level in 1..=6 {
        let hash = "#".repeat(level);
        let marker = format!("{hash} ");
        if let Some(rest) = line.strip_prefix(&marker) {
            return (&line[..level + 1], rest);
        }
    }
    ("", line)
}

///
/// Off by default (it is per keystroke); on when chasing "did that key even
/// reach the buffer". `insert` is always escaped, because the whole point is to
/// tell `\n` from `\r` from `\\n` at a glance.
pub fn edit_log_enabled() -> bool {
    env_flag("TYPORS_EDIT_LOG")
}

/// Escape text for one log line: `\n`, `\r`, `\t` and any other control
/// character become visible escapes instead of breaking or hiding in the line.
pub fn escape_insert(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:04X}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// LF-normalise text on its way into the buffer.
///
/// A `\r` is not a line break anywhere downstream: `pulldown-cmark` does not end
/// a line on it, the renderer draws it as a control glyph, and the span table
/// keeps it as plain text. So a CR that reaches the buffer becomes a character
/// that is *invisible and does not break the line* — which is exactly what "the
/// newline did not appear" looks like from the outside, while the byte count
/// still goes up. The only place that can be made impossible is the one place the
/// buffer is written: here. CRLF and lone CR both become LF.
///
/// This is also the fix for pasting a CRLF document: it used to insert real CRs,
/// which render as nothing, instead of line breaks.
pub fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Where an insertion lands: a live selection is replaced, otherwise it goes in
/// at the caret.
///
/// Extracted because this one rule decides whether ten Enters make ten newlines
/// or keep overwriting the same one, and it is testable without an app context.
pub fn insertion_range(caret: Option<usize>, selection: Option<Range<usize>>) -> Range<usize> {
    match selection {
        Some(range) if !range.is_empty() => range,
        _ => {
            let at = caret.unwrap_or(0);
            at..at
        }
    }
}

actions!(
    typors_editor,
    [
        Backspace,
        DeleteForward,
        Newline,
        CaretLeft,
        CaretRight,
        CaretUp,
        CaretDown,
        LineStart,
        LineEnd,
        DocStart,
        DocEnd,
        ExtendLeft,
        ExtendRight,
        ExtendUp,
        ExtendDown,
        ExtendLineStart,
        ExtendLineEnd,
        SelectAllText,
        CopySelection,
        CutSelection,
        PasteClipboard,
        UndoEdit,
        RedoEdit,
    ]
);

/// One keyboard caret movement. Vertical moves are geometric (they keep an x),
/// horizontal moves are textual (they walk char boundaries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    Up,
    Down,
    /// Start of the caret block's source line.
    LineStart,
    /// End of the caret block's source line (before its trailing newline).
    LineEnd,
    DocStart,
    DocEnd,
}

/// One text mutation, in the form group 5's undo needs: re-applying
/// `src.start..src.start + inserted.len()` with `removed` reverses it.
#[derive(Clone, Debug)]
pub struct EditRecord {
    /// Range replaced in the source buffer, after char-boundary snapping.
    pub src: Range<usize>,
    pub removed: String,
    pub inserted: String,
    pub caret_before: Option<usize>,
    pub caret_after: Option<usize>,
    /// `Document::revision` after the edit: a layout cache invalidates on this.
    pub revision: u64,
}

/// Everything the editor knows about its single caret, all derived from
/// [`MarkdownView::caret`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretState {
    /// Source byte offset. The only stored truth.
    pub src: usize,
    /// The block that owns it; exactly this block is in the source projection.
    pub block_idx: usize,
    /// Byte offset inside that block's painted text.
    pub display_byte: usize,
    /// Caret rectangle in window coordinates (from the current layout).
    pub bounds: Option<Bounds<Pixels>>,
}

/// Typora live preview: every block is rendered, including the block under the
/// caret. Only chart/mermaid fences keep their raw source while focused, because
/// they must not collapse into an SVG while the user is typing inside them.
pub fn mode_for(doc: &Document, caret: Option<usize>) -> LayoutMode {
    let source_block = caret.and_then(|src| {
        let idx = doc.block_at_src(src)?;
        (doc.block_is_rich_fence(idx)).then_some(idx)
    });
    LayoutMode { source_block }
}

/// UTF-8 byte offset -> UTF-16 code units. `EntityInputHandler` speaks UTF-16,
/// everything else in this crate speaks UTF-8 bytes (`input.rs:15-82`).
pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..snap_str_floor(text, byte)].encode_utf16().count()
}

/// UTF-16 code units -> UTF-8 byte offset.
pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut count = 0usize;
    for (byte, ch) in text.char_indices() {
        if count >= utf16 {
            return byte;
        }
        count += ch.len_utf16();
    }
    text.len()
}

fn snap_str_floor(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

/// The document view: one buffer, two projections, one caret.
pub struct MarkdownView {
    doc: Document,
    theme: EditorTheme,
    focus_handle: FocusHandle,
    /// The caret: a byte offset into the source buffer, or `None` before the
    /// first click (a document with no caret is drawn fully rendered).
    caret: Option<usize>,
    /// Blink phase: true means the caret is painted this frame.
    caret_visible: bool,
    blink_enabled: bool,
    blink_interval_ms: u64,
    blink_started: bool,
    /// Bumped by every activity reset; a sleeping timer can observe it.
    blink_generation: u64,
    completion: Option<crate::completion::CompletionState>,
    completion_enabled: bool,
    completion_delay_ms: u64,
    completion_generation: u64,
    /// The selection's fixed end, also a source byte offset. The live selection is
    /// `anchor..caret` whenever the two differ; `anchor == caret` (or `None`) is a
    /// collapsed caret. Deliberately *two* source offsets and nothing else: the
    /// highlight, the clipboard text and the UTF-16 answer the platform gets are
    /// all derived, so none of them can disagree after a re-parse.
    anchor: Option<usize>,
    /// The undo/redo stacks ([`crate::selection::History`]).
    history: History,
    /// The last copy/cut, kept as a shadow of the platform clipboard so `Ctrl+V`
    /// works inside the app even on a platform read that returns nothing
    /// (findings.md U2). The platform value always wins when it exists.
    clipboard: Option<String>,
    /// A composition that started on a non-empty selection: the platform only
    /// tells us the collapsed anchor, so the range that the commit must replace is
    /// remembered here (「选中后提交必须替换整个选区」).
    marked_selection: Option<Range<usize>>,
    /// True between a left press inside the document and its release: mouse moves
    /// then extend the selection instead of moving the caret.
    dragging: bool,
    /// x to keep while walking vertically; cleared by any horizontal move.
    desired_x: Option<Pixels>,
    /// Layout of the last painted frame, in window coordinates. This is what
    /// hit testing (`DocLayout::hit`) and `--dump-layout` read.
    layout: Option<DocLayout>,
    /// Image state and the math engine used by the layout and paint passes.
    media: MediaContext,
    /// Set when the buffer or the caret moved after `layout` was produced, so
    /// window mode re-lays-out exactly once per frame (`request_layout`) instead
    /// of once per edit *and* once per frame. Headless callers call
    /// [`MarkdownView::sync_layout`] to clear it.
    stale: bool,
    /// The live IME composition (design D4). `None` whenever no composition is in
    /// progress; the buffer never holds it, [`crate::ime`] does.
    marked: Option<MarkedText>,
    /// Vertical scroll state. Scrolling does not shift any layout math: the
    /// element's bounds already carry the offset, and `paint` translates the
    /// layout by it, so the published coordinates stay window coordinates.
    ///
    /// It *is* written by the view now, in one place:
    /// [`MarkdownView::scroll_caret_into_view`], which `request_layout` calls once
    /// per caret position so the caret follows the text it types
    /// (「回车后光标不跟随」).
    scroll_handle: ScrollHandle,
    /// The `(revision, caret, anchor)` the last scroll-into-view correction was made
    /// for. A wheel scroll changes none of them, so the correction cannot fight the
    /// user's own scrolling.
    scroll_epoch: Option<(u64, Option<usize>, Option<usize>)>,
    /// Scroll anchor: `(caret block, its document top, scroll offset)` from the
    /// previous frame. When the same block stays under the caret, a change in its
    /// top (an image above finished loading, a code fence appeared/disappeared)
    /// is compensated in the scroll offset so the page does not jump vertically
    /// while typing.
    scroll_anchor: Option<(usize, f32, f32)>,
    dump: Option<DumpSink>,
    /// Set by `--dump-ime`: the probe runs once, against the first painted frame.
    ime_probe: Option<ImeSink>,
    /// Set by `--dump-select`: same idea, group 5's probe. It needs a `Window`
    /// only for the two `EntityInputHandler` calls it drives (they are `&mut
    /// Window`), but it is implemented that way so that what it asserts is the
    /// geometry the user's eyes get, not a second implementation.
    select_probe: Option<SelectSink>,
    /// Set by `--dump-perf`: the task 7.1 sweep, run once against the first frame
    /// (it needs the window text system, which only exists inside a frame).
    perf_probe: Option<PerfSink>,
    /// The sweep itself, injected because it lives in the probe crate.
    perf_runner: Option<PerfProbeFn>,
    /// Set by `--dump-media`: rich-content assertions, run once.
    media_probe: Option<MediaSink>,
    media_runner: Option<MediaProbeFn>,
    /// How many rich-content commands the last paint emitted (probe evidence).
    painted_objects: usize,
    /// Draw the real `gpui-fps` HUD over the page (`--fps`).
    show_fps: bool,
    last_edit: Option<EditRecord>,
    /// The entry point of the most recent buffer change (see [`EditTrigger`]).
    /// Kept beside `last_edit` rather than inside it: `EditRecord` is the undo
    /// unit and must not grow a field that has nothing to do with undoing.
    last_trigger: Option<EditTrigger>,
    edits: usize,
    /// Cross-frame shaped-fragment cache shared with every [`WindowMetrics`] the
    /// frames create (task 7.1, optimisation ①).
    shape_cache: Arc<ShapeCache>,
    /// Cross-frame per-block geometry cache (task 7.1, 块级缓存).
    block_cache: BlockCache,
    /// The metrics of the frame that painted last. Kept because a `Window` only
    /// exists inside a frame, so anything that has to measure afterwards (the
    /// `--dump-perf` sweep) needs the text system the frame used.
    frame_metrics: Option<Arc<WindowMetrics>>,
    /// `TYPORS_SCROLL` has been applied (see `env_f32`), so a user's own scroll is
    /// not overwritten every frame.
    scroll_applied: bool,
    /// `TYPORS_CULL_LOG`: print the culling band per frame.
    cull_log: bool,
    /// The visible editor band, set by the shell. `None` means "the whole
    /// window", which is what the headless probes use.
    editor_viewport: Option<EditorViewport>,
    /// Grab offset (pixels from the thumb top) while dragging the scrollbar.
    scrollbar_drag: Option<f32>,
    /// The fence-language editor, when it is open for the caret's code block.
    fence_edit: Option<FenceLanguageEdit>,
    /// Process-wide switches for the streaming pipeline and the persistent
    /// cache (`TYPORS_STREAMING`, `TYPORS_CACHE`).
    features: typors_cache::features::Features,
    /// Receiver for the background whole-file parse of a progressively opened
    /// document. Applying it is deferred to a frame on the UI thread.
    parse_rx: Option<std::sync::mpsc::Receiver<Vec<typors_core::doc::Block>>>,
    /// Probes that compare whole-document layouts can pin streaming off without
    /// touching the process-wide switch.
    streaming_override: Option<bool>,
}

impl MarkdownView {
    pub fn new(doc: Document, cx: &mut Context<Self>) -> Self {
        let blank = doc.len_bytes() == 0;
        Self {
            doc,
            theme: EditorTheme::default(),
            focus_handle: cx.focus_handle(),
            caret: blank.then_some(0),
            caret_visible: true,
            blink_enabled: true,
            blink_interval_ms: 530,
            blink_started: false,
            blink_generation: 0,
            completion: None,
            completion_enabled: true,
            completion_delay_ms: 150,
            completion_generation: 0,
            anchor: blank.then_some(0),
            history: History::new(),
            clipboard: None,
            marked_selection: None,
            dragging: false,
            desired_x: None,
            layout: None,
            media: MediaContext::default(),
            stale: true,
            marked: None,
            scroll_handle: ScrollHandle::new(),
            scroll_epoch: None,
            scroll_anchor: None,
            dump: None,
            ime_probe: None,
            select_probe: None,
            perf_probe: None,
            perf_runner: None,
            media_probe: None,
            media_runner: None,
            painted_objects: 0,
            show_fps: false,
            last_edit: None,
            last_trigger: None,
            edits: 0,
            shape_cache: Arc::new(ShapeCache::new()),
            block_cache: BlockCache::new(),
            frame_metrics: None,
            scroll_applied: false,
            cull_log: env_flag("TYPORS_CULL_LOG"),
            editor_viewport: None,
            scrollbar_drag: None,
            fence_edit: None,
            features: typors_cache::features::Features::global(),
            parse_rx: None,
            streaming_override: None,
        }
    }

    /// Pin streaming on or off for controlled comparisons (probes and tests).
    pub fn set_streaming_override(&mut self, value: Option<bool>) {
        self.streaming_override = value;
    }

    /// Finish a progressively opened document on a background thread and hand
    /// the block table back to the UI. Does nothing once the document is whole.
    pub fn start_progressive_parse(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.doc.is_parsing() || !self.streaming_enabled() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let blocks = typors_core::doc::Document::parse_blocks_for(&text);
            let _ = sender.send(blocks);
        });
        self.parse_rx = Some(receiver);
        cx.notify();
    }

    /// Apply a finished background parse. Returns true when the document grew.
    /// Called once per frame; while the parse is still running the caller asks
    /// for another frame so it can land promptly.
    pub fn poll_progressive_parse(&mut self) -> bool {
        let Some(receiver) = &self.parse_rx else {
            return false;
        };
        match receiver.try_recv() {
            Ok(blocks) => {
                self.parse_rx = None;
                if self.doc.is_parsing() {
                    self.doc.finish_progressive(blocks);
                    self.stale = true;
                    return true;
                }
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.parse_rx = None;
                false
            }
        }
    }

    /// True while a background parse is still pending (the caller should keep
    /// requesting frames).
    pub fn parse_pending(&self) -> bool {
        self.parse_rx.is_some()
    }

    /// Whether viewport-first layout with background measurement is enabled.
    pub fn streaming_enabled(&self) -> bool {
        self.streaming_override.unwrap_or(self.features.streaming)
    }

    /// Whether the persistent render cache is enabled.
    pub fn cache_enabled(&self) -> bool {
        self.features.cache
    }

    pub fn with_dump(doc: Document, sink: DumpSink, cx: &mut Context<Self>) -> Self {
        Self {
            dump: Some(sink),
            ..Self::new(doc, cx)
        }
    }

    /// A view that knows which file it was opened from, so relative image paths
    /// resolve against that file's directory.
    pub fn new_at(doc: Document, source: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let doc_dir = source.and_then(|path| path.parent().map(|dir| dir.to_path_buf()));
        let mut view = Self::new(doc, cx);
        view.media = MediaContext::new(doc_dir);
        view
    }

    /// Poll the image asset system once per layout pass. Newly decoded images
    /// bump the media generation, which invalidates the block geometry cache.
    pub fn refresh_media(&mut self, window: &mut Window, cx: &mut App) {
        self.media.refresh(&self.doc, window, cx);
    }

    pub fn media(&self) -> &MediaContext {
        &self.media
    }

    /// Rich-content paint commands emitted by the last frame (probe evidence).
    pub fn painted_objects(&self) -> usize {
        self.painted_objects
    }

    /// Resolve relative image paths against this directory.
    pub fn set_doc_dir(&mut self, dir: Option<PathBuf>) {
        self.media = MediaContext::new(dir);
    }

    /// Attach the document's disk cache (images, chart/Mermaid rasters).
    pub fn set_document_cache(&mut self, cache: Option<typors_cache::store::DocumentCache>) {
        self.media.set_document_cache(cache);
    }

    /// A view whose first painted frame also runs the rich-content probe.
    pub fn with_media_probe(
        doc: Document,
        sink: MediaSink,
        runner: MediaProbeFn,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            media_probe: Some(sink),
            media_runner: Some(runner),
            ..Self::new(doc, cx)
        }
    }

    /// A view whose first painted frame also runs the `--dump-ime` probe: the IME
    /// callbacks need a real `Window`, which only exists inside the frame.
    pub fn with_ime_probe(doc: Document, sink: ImeSink, cx: &mut Context<Self>) -> Self {
        Self {
            ime_probe: Some(sink),
            ..Self::new(doc, cx)
        }
    }

    /// A view whose first painted frame also runs the `--dump-select` probe
    /// (group 5). Same reason as [`MarkdownView::with_ime_probe`]: two of the
    /// assertions drive `EntityInputHandler`, which takes `&mut Window`.
    pub fn with_select_probe(doc: Document, sink: SelectSink, cx: &mut Context<Self>) -> Self {
        Self {
            select_probe: Some(sink),
            ..Self::new(doc, cx)
        }
    }

    /// A view whose first painted frame also runs the task 7.1 performance sweep.
    /// Same reason again: it measures with the window text system, which only
    /// exists inside a frame.
    pub fn with_perf_probe(
        doc: Document,
        sink: PerfSink,
        runner: PerfProbeFn,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            perf_probe: Some(sink),
            perf_runner: Some(runner),
            ..Self::new(doc, cx)
        }
    }

    /// Draw the `gpui-fps` HUD over the page (window mode's `--fps`).
    pub fn with_fps(doc: Document, cx: &mut Context<Self>) -> Self {
        Self {
            show_fps: true,
            ..Self::new(doc, cx)
        }
    }

    /// Apply persisted settings: theme, page metrics and blink.
    pub fn apply_settings(&mut self, settings: &typors_settings::Settings, cx: &mut Context<Self>) {
        self.theme = typors_render::EditorTheme::from_settings(settings);
        self.completion_enabled = settings.editing.completion_enabled;
        self.completion_delay_ms = settings.editing.completion_delay_ms as u64;
        if !self.completion_enabled {
            self.completion = None;
        }
        self.set_blink(
            settings.editing.blink_enabled,
            settings.editing.blink_interval_ms as u64,
            cx,
        );
        self.stale = true;
        self.layout = None;
        cx.notify();
    }

    pub fn completion(&self) -> Option<&crate::completion::CompletionState> {
        self.completion.as_ref()
    }

    pub fn completion_anchor(&self) -> Option<(Point<Pixels>, Pixels)> {
        let state = self.completion.as_ref()?;
        let layout = self.layout.as_ref()?;
        let (block, byte) = layout.display_byte_for_src(&self.doc, state.start)?;
        let bounds = layout.caret_bounds(block, byte)?;
        Some((bounds.origin, bounds.size.height))
    }

    fn schedule_completion(&mut self, cx: &mut Context<Self>) {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        let generation = self.completion_generation;
        self.completion = None;
        if !self.completion_enabled || self.marked.is_some() {
            return;
        }
        let delay = self.completion_delay_ms;
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_entity, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(delay))
                .await;
            let _ = weak.update(cx, |view, cx| {
                if view.completion_generation != generation || view.marked.is_some() {
                    return;
                }
                view.completion = view
                    .caret
                    .and_then(|caret| crate::completion::suggest(&view.doc, caret));
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether the fence-language editor is open.
    pub fn fence_language(&self) -> Option<&FenceLanguageEdit> {
        self.fence_edit.as_ref()
    }

    fn fence_items_for(&self, filter: &str) -> Vec<crate::completion::CompletionItem> {
        let mut items = Vec::new();
        if filter.trim().is_empty() {
            items.push(crate::completion::plain_language_item());
        }
        items.extend(crate::completion::language_items(filter.trim()));
        items
    }

    /// Items currently shown by the fence-language editor.
    pub fn fence_language_items(&self) -> Vec<crate::completion::CompletionItem> {
        self.fence_edit
            .as_ref()
            .map(|edit| self.fence_items_for(&edit.filter))
            .unwrap_or_default()
    }

    pub fn fence_language_selected(&self) -> Option<usize> {
        self.fence_edit.as_ref().map(|edit| edit.selected)
    }

    /// Open the language editor for a code block, starting from its info string.
    pub fn begin_fence_language_edit(&mut self, block_idx: usize, cx: &mut Context<Self>) {
        let Some(block) = self.doc.blocks().get(block_idx) else {
            return;
        };
        let info = match &block.kind {
            BlockKind::FencedCode { info } => info.trim().to_string(),
            _ => return,
        };
        let items = self.fence_items_for(&info);
        let selected = items
            .iter()
            .position(|item| item.label.eq_ignore_ascii_case(&info))
            .unwrap_or(0);
        self.dismiss_completion();
        if std::env::var("TYPORS_PILL_DEBUG").is_ok() {
            eprintln!(
                "pill: editor opened block={block_idx} info={info:?} items={} selected={selected}",
                items.len()
            );
        }
        self.fence_edit = Some(FenceLanguageEdit {
            block_idx,
            filter: info,
            selected,
            fresh: true,
        });
        cx.notify();
    }

    /// Consume typed text as the filter while the editor is open.
    pub fn fence_language_typed(&mut self, text: &str) {
        let Some(edit) = &mut self.fence_edit else {
            return;
        };
        let mut changed = false;
        for ch in text.chars() {
            if ch.is_ascii_alphanumeric() || matches!(ch, '+' | '#' | '-' | '_' | '.') {
                if edit.fresh {
                    edit.filter.clear();
                    edit.fresh = false;
                }
                edit.filter.push(ch.to_ascii_lowercase());
                changed = true;
            }
        }
        if changed {
            edit.selected = 0;
        }
    }

    pub fn fence_language_backspace(&mut self) -> bool {
        let Some(edit) = &mut self.fence_edit else {
            return false;
        };
        edit.fresh = false;
        let changed = edit.filter.pop().is_some();
        if changed {
            edit.selected = 0;
        }
        changed
    }

    pub fn fence_language_move(&mut self, delta: isize) {
        let Some(filter) = self.fence_edit.as_ref().map(|edit| edit.filter.clone()) else {
            return;
        };
        let items = self.fence_items_for(&filter);
        if items.is_empty() {
            return;
        }
        let len = items.len();
        if let Some(edit) = &mut self.fence_edit {
            edit.selected = (edit.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    pub fn fence_language_cancel(&mut self) {
        self.fence_edit = None;
    }

    pub fn fence_language_choose(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.fence_edit {
            edit.selected = index;
        }
        self.fence_language_commit(cx);
    }

    /// Rewrite the fence info with the selected language and close the editor.
    pub fn fence_language_commit(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(edit) = self.fence_edit.take() else {
            return false;
        };
        let items = self.fence_items_for(&edit.filter);
        let chosen = items
            .get(edit.selected)
            .map(|item| item.label.clone())
            .unwrap_or_else(|| edit.filter.clone());
        let language = if chosen == "(无语言)" {
            String::new()
        } else {
            chosen
        };
        self.set_fence_language(edit.block_idx, &language);
        cx.notify();
        true
    }

    /// Replace the info string on a code block's opening line, keeping the caret
    /// inside the block and the edit undoable.
    pub fn set_fence_language(&mut self, block_idx: usize, language: &str) {
        let Some(block) = self.doc.blocks().get(block_idx).cloned() else {
            return;
        };
        if !matches!(block.kind, BlockKind::FencedCode { .. }) {
            return;
        }
        let raw = self.doc.rope().slice(block.src.clone()).to_string();
        let Some(range) = crate::completion::fence_info_range(&raw, block.src.start) else {
            return;
        };
        let (info_start, info_end) = (range.start, range.end);
        let replacement = language.trim();
        if self
            .doc
            .rope()
            .slice(info_start..info_end)
            .to_string()
            == replacement
        {
            return;
        }
        let caret = self.caret;
        let old_len = info_end.saturating_sub(info_start);
        self.apply_edit_internal(
            info_start..info_end,
            replacement,
            HistoryAction::Record,
            EditTrigger::Format,
        );
        let new_end = info_start + replacement.len();
        if let Some(caret) = caret {
            let adjusted = if caret >= info_end {
                caret - old_len + replacement.len()
            } else if caret > info_start {
                new_end
            } else {
                caret
            };
            self.caret = Some(self.doc.snap_floor(adjusted.min(self.doc.len_bytes())));
            self.anchor = self.caret;
            self.desired_x = None;
        }
        self.dismiss_completion();
    }

    /// The language chip for the caret's code block: (block, bounds, label, editing).
    pub fn fence_pill(&self) -> Option<(usize, Bounds<Pixels>, String, bool)> {
        let viewport = self.editor_viewport?;
        let caret = self.caret?;
        let block_idx = self.doc.block_at_src(caret)?;
        let block = self.doc.blocks().get(block_idx)?;
        let info = match &block.kind {
            BlockKind::FencedCode { info } => info.trim().to_string(),
            _ => return None,
        };
        let bounds = self.layout.as_ref()?.blocks.get(block_idx)?.bounds;
        let top = bounds.origin.y.as_f32();
        if top < viewport.top - 1.0 || top > viewport.top + viewport.height - 24.0 {
            return None;
        }
        let label = if info.is_empty() {
            "text".to_string()
        } else {
            info
        };
        Some((block_idx, bounds, label, self.fence_edit.is_some()))
    }

    pub fn dismiss_completion(&mut self) {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        self.completion = None;
    }

    pub fn move_completion(&mut self, delta: isize) {
        if let Some(state) = &mut self.completion {
            state.move_selection(delta);
        }
    }

    pub fn accept_completion(&mut self) -> bool {
        let Some(state) = self.completion.take() else {
            return false;
        };
        let item = state.current().clone();
        let start = state.start;
        let end = state.end;
        self.apply_edit_internal(
            start..end,
            &item.insert,
            HistoryAction::Record,
            EditTrigger::Format,
        );
        self.caret = Some((start + item.caret).min(self.doc.len_bytes()));
        self.anchor = self.caret;
        self.stale = true;
        true
    }

    /// Page width for the current viewport, including Typora's 1400/1800 px
    /// breakpoints on top of the user's preference.
    pub fn page_width_for(&self, viewport_width: f32) -> f32 {
        if (self.theme.page_width - 860.0).abs() < 0.5 {
            if viewport_width >= 1800.0 {
                1200.0
            } else if viewport_width >= 1400.0 {
                1024.0
            } else {
                860.0
            }
        } else {
            self.theme.page_width
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    /// Apply blink settings without restarting the view.
    pub fn set_blink(&mut self, enabled: bool, interval_ms: u64, cx: &mut Context<Self>) {
        self.apply_blink_config(enabled, interval_ms);
        cx.notify();
    }

    /// Any activity makes the caret visible again and starts a new phase.
    pub fn reset_blink(&mut self) {
        self.caret_visible = true;
        self.blink_generation = self.blink_generation.wrapping_add(1);
    }

    pub fn caret_visible(&self) -> bool {
        self.caret_visible
    }

    /// Blink configuration without a `Context`; `set_blink` is this plus notify.
    pub fn apply_blink_config(&mut self, enabled: bool, interval_ms: u64) {
        self.blink_enabled = enabled;
        self.blink_interval_ms = interval_ms.clamp(300, 1200);
        if !enabled {
            self.caret_visible = true;
        }
        self.reset_blink();
    }

    pub fn blink_enabled(&self) -> bool {
        self.blink_enabled
    }

    pub fn blink_interval_ms(&self) -> u64 {
        self.blink_interval_ms
    }

    pub fn blink_generation(&self) -> u64 {
        self.blink_generation
    }

    /// Flip one blink phase without waiting for the wall clock. Only probes and
    /// unit tests call this; the real timer uses the same field.
    #[doc(hidden)]
    pub fn advance_blink_phase(&mut self) {
        if self.blink_enabled {
            self.caret_visible = !self.caret_visible;
        } else {
            self.caret_visible = true;
        }
    }

    /// Replace the whole document (New/Open) and reset every piece of state that
    /// points into the old buffer. The one caret that remains is at offset 0.
    pub fn replace_document(&mut self, doc: Document, cx: &mut Context<Self>) {
        self.doc = doc;
        self.caret = Some(0);
        self.anchor = Some(0);
        self.history = History::new();
        self.marked = None;
        self.marked_selection = None;
        self.dragging = false;
        self.desired_x = None;
        self.layout = None;
        self.stale = true;
        self.scroll_epoch = None;
        self.scroll_anchor = None;
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
        self.block_cache = BlockCache::new();
        self.painted_objects = 0;
        cx.notify();
    }

    pub fn theme(&self) -> &EditorTheme {
        &self.theme
    }

    /// Tell the editor which window band its scroll container occupies (the
    /// shell calls this each render; probes leave it unset for the full window).
    pub fn set_editor_viewport(&mut self, viewport: Option<EditorViewport>) {
        self.editor_viewport = viewport;
    }

    /// The visible editor band, falling back to the whole window.
    pub fn viewport_rect(&self, window_height: f32) -> EditorViewport {
        self.editor_viewport.unwrap_or(EditorViewport {
            top: 0.0,
            height: window_height.max(1.0),
        })
    }

    /// Scrollbar geometry in window coordinates, or None when the page fits.
    fn scrollbar_bounds(
        &self,
        window_height: f32,
        window_width: f32,
    ) -> Option<typors_render::ScrollbarGeometry> {
        let layout = self.layout.as_ref()?;
        let viewport = self.viewport_rect(window_height);
        if viewport.height <= 0.0 {
            return None;
        }
        typors_render::scrollbar_geometry(
            window_width,
            layout.size.height.as_f32(),
            viewport.top,
            viewport.height,
            self.scroll_offset_y(),
        )
    }

    /// Move the scroll thumb so its top lands at `thumb_top` (window coordinates).
    fn scroll_to_scrollbar_thumb(&mut self, thumb_top: f32, window_height: f32) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let viewport = self.viewport_rect(window_height);
        if let Some(scroll) = typors_render::scroll_for_scrollbar_thumb_top(
            layout.size.height.as_f32(),
            viewport.top,
            viewport.height,
            thumb_top,
        ) {
            self.set_scroll_offset_y(scroll);
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The scroll container's handle (group 7 drives viewport culling from it).
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll_handle
    }

    /// Layout of the last painted frame, in window coordinates.
    pub fn layout(&self) -> Option<&DocLayout> {
        self.layout.as_ref()
    }

    pub fn caret(&self) -> Option<usize> {
        self.caret
    }

    /// The live IME composition, if the user is in the middle of one.
    pub fn marked(&self) -> Option<&MarkedText> {
        self.marked.as_ref()
    }

    pub fn marked_text(&self) -> Option<&str> {
        self.marked.as_ref().map(|m| m.text.as_str())
    }

    /// The block in the source projection for the current caret.
    pub fn caret_block(&self) -> Option<usize> {
        self.caret.and_then(|src| self.doc.block_at_src(src))
    }

    pub fn mode(&self) -> LayoutMode {
        self.projection_mode()
    }

    /// During an IME composition the uncommitted string only exists in the
    /// platform callback, not in the rope. Paint that one block as source so the
    /// composition can be spliced and underlined; once it commits, live preview
    /// resumes.
    fn projection_mode(&self) -> LayoutMode {
        if self.marked.is_some() {
            return LayoutMode {
                source_block: self.caret.and_then(|src| self.doc.block_at_src(src)),
            };
        }
        mode_for(&self.doc, self.caret)
    }

    pub fn last_edit(&self) -> Option<&EditRecord> {
        self.last_edit.as_ref()
    }

    /// Which entry point made the most recent buffer change.
    pub fn last_edit_trigger(&self) -> Option<EditTrigger> {
        self.last_trigger
    }

    pub fn edit_count(&self) -> usize {
        self.edits
    }

    /// True when layout() predates the last edit or caret move. Window mode
    /// clears it in the next frame's `request_layout`; a headless caller calls
    /// [MarkdownView::sync_layout].
    pub fn is_stale(&self) -> bool {
        self.stale
    }
}

// ------------------------------------------------------- caret scroll-into-view

/// The rectangle the editor's scroll container actually shows, in window
/// coordinates. The window is taller than the editor: the menu bar and status bar
/// take a fixed 28 px each, and caret scrolling must respect the real band or it
/// leaves the caret inside the bars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorViewport {
    pub top: f32,
    pub height: f32,
}

/// The language editor at the top-right of a fenced code block.
///
/// The box starts as a display of the block's info string; clicking it turns it
/// into a filter over the language list. Committing rewrites the opening fence
/// line, so the block keeps its content and the caret stays inside it.
#[derive(Clone, Debug)]
pub struct FenceLanguageEdit {
    pub block_idx: usize,
    /// What the user has typed so far (the filter, also the initial language).
    pub filter: String,
    pub selected: usize,
    /// The first typed character replaces the pre-filled language instead of
    /// appending to it (select-all semantics for a small picker).
    pub fresh: bool,
}

/// What one caret scroll-into-view correction did, in pixels from the top of the
/// document.
///
/// Reported by `--dump-newlines` (N2/N3) and printable under `TYPORS_CULL_LOG`, so
/// "the caret follows the text" is an observation and not a hope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretScroll {
    /// Scroll offset before the correction.
    pub from: f32,
    /// Scroll offset after it.
    pub to: f32,
    /// The caret's line box, in document coordinates.
    pub caret_top: f32,
    pub caret_bottom: f32,
}

impl CaretScroll {
    /// How far the page moved, in pixels. Negative = scrolled up.
    pub fn moved(&self) -> f32 {
        self.to - self.from
    }
}

/// The smallest scroll offset that puts `caret_top..caret_bottom` inside
/// `scroll..scroll + viewport_height`, keeping `margin` above and below.
///
/// `None` means "leave the scroll alone", which is the answer for a caret that is
/// already visible: typing must never scroll a document the user is reading.
///
/// A pure function on purpose — the whole policy is unit-testable without a
/// window, and the frame path and `--dump-newlines` both go through it.
pub fn caret_scroll_target(
    caret_top: f32,
    caret_bottom: f32,
    scroll: f32,
    page_height: f32,
    viewport_height: f32,
    margin: f32,
) -> Option<f32> {
    if !(scroll.is_finite() && viewport_height.is_finite() && viewport_height > 0.) {
        return None;
    }
    let max_scroll = (page_height - viewport_height).max(0.);
    // A margin wider than half the viewport would invert the band.
    let margin = margin.max(0.).min(viewport_height / 2.);
    let band_top = scroll + margin;
    let band_bottom = scroll + viewport_height - margin;
    let target = if caret_bottom > band_bottom {
        // Below the band: move down by exactly how far the caret's bottom edge
        // sticks out, so its whole line box lands on screen.
        scroll + (caret_bottom - band_bottom)
    } else if caret_top < band_top {
        // Above it: same idea, upwards.
        scroll - (band_top - caret_top)
    } else {
        return None;
    };
    let target = target.clamp(0., max_scroll);
    // Sub-pixel differences are not worth a scroll (and would spin the frame).
    ((target - scroll).abs() > 0.5).then_some(target)
}

impl MarkdownView {
    /// Where the caret's bar goes in `layout`: which block, and which byte of that
    /// block's painted text.
    ///
    /// Shared by the caret quad in `prepaint`, the IME candidate anchor and the
    /// scroll decision, so all three can never disagree about where the caret is.
    pub fn caret_display_position(&self, layout: &DocLayout) -> Option<(usize, usize)> {
        match (&self.marked, self.caret) {
            // While composing, the caret is drawn at the IME's own cursor inside the
            // composition string; the buffer caret has not moved.
            (Some(marked), _) => {
                layout.display_byte_for_composite(&self.doc, marked.composite_caret())
            }
            (None, Some(src)) => self.caret_display_byte(layout, src),
            (None, None) => None,
        }
    }

    /// The scroll amount: pixels from the top of the document, `>= 0`.
    pub fn scroll_offset_y(&self) -> f32 {
        -self.scroll_handle.offset().y.as_f32()
    }

    /// Set the scroll amount (`0` = the top of the document).
    ///
    /// The scroll container clamps this to what the document can actually scroll
    /// (`Interactivity::prepaint` does it, after every `request_layout`), so an
    /// out-of-range value is corrected rather than obeyed.
    pub fn set_scroll_offset_y(&self, scroll: f32) {
        self.scroll_handle
            .set_offset(point(px(0.), px(-scroll.max(0.))));
    }

    /// The caret's line box in **document** coordinates (top of the page = 0).
    ///
    /// The published layout is in window coordinates, so this undoes the
    /// translation `paint` applied. Scroll-into-view has to work in document
    /// coordinates: window coordinates move with the very scroll the correction is
    /// about to change.
    pub fn caret_bounds_in_document(&self) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let mut rect = self.caret_bounds()?;
        rect.origin.x = px(rect.origin.x.as_f32() - layout.origin.x.as_f32());
        rect.origin.y = px(rect.origin.y.as_f32() - layout.origin.y.as_f32());
        Some(rect)
    }

    /// Bring the caret into view if `layout` puts it outside the visible band.
    ///
    /// `layout` must be in document coordinates — what [`MarkdownView::layout_now`]
    /// and [`MarkdownView::layout_now_cached`] produce with the origin at `(0, 0)`,
    /// which is exactly what `request_layout` measures. Using the *complete* layout,
    /// not the set of fragments the viewport cull kept, is deliberate: the cull is a
    /// paint-time filter (task 7.1) and must never decide whether the caret is
    /// visible.
    pub fn scroll_caret_into_view(
        &mut self,
        layout: &DocLayout,
        viewport_height: f32,
    ) -> Option<CaretScroll> {
        let (block_idx, display_byte) = self.caret_display_position(layout)?;
        let rect = layout.caret_bounds(block_idx, display_byte)?;
        let margin = layout
            .blocks
            .get(block_idx)
            .map_or(self.theme.body_size * self.theme.line_ratio, |block| {
                block.line_height.as_f32()
            });
        let scroll = self.scroll_offset_y();
        let target = caret_scroll_target(
            rect.top().as_f32(),
            rect.bottom().as_f32(),
            scroll,
            layout.size.height.as_f32(),
            viewport_height,
            margin,
        )?;
        self.set_scroll_offset_y(target);
        Some(CaretScroll {
            from: scroll,
            to: target,
            caret_top: rect.top().as_f32(),
            caret_bottom: rect.bottom().as_f32(),
        })
    }

    /// [`MarkdownView::scroll_caret_into_view`] on a freshly measured layout: the
    /// same two steps `request_layout` runs. Headless callers (and the probes) use
    /// this; it needs a frame's metrics, i.e. a window text system.
    pub fn scroll_caret_into_view_now(&mut self, viewport_height: f32) -> Option<CaretScroll> {
        let metrics: Arc<dyn Metrics> = self.frame_metrics.clone()?;
        let width = self.layout.as_ref().map_or(px(1000.), |l| l.size.width);
        let layout = self.layout_now_cached(metrics, point(px(0.), px(0.)), width);
        self.scroll_caret_into_view(&layout, viewport_height)
    }

    /// Consult [`MarkdownView::scroll_caret_into_view`] at most once per caret
    /// position.
    ///
    /// The window path calls this every frame, and the guard is what makes that
    /// safe: a scroll the *user* performed with the wheel changes neither the
    /// buffer revision nor the caret, so it must not be undone. An edit or a caret
    /// move changes the epoch, and then the caret is brought back into view.
    fn scroll_for_caret_if_moved(
        &mut self,
        layout: &DocLayout,
        viewport_height: f32,
    ) -> Option<CaretScroll> {
        let epoch = (self.doc.revision(), self.caret, self.anchor);
        if self.scroll_epoch == Some(epoch) {
            return None;
        }
        self.scroll_epoch = Some(epoch);
        self.scroll_caret_into_view(layout, viewport_height)
    }

    /// Compensate a change in the caret block's document top by the same delta in
    /// the scroll offset. Typing usually leaves the block top alone; an image
    /// above finishing its decode, or a code fence appearing, does not.
    fn apply_scroll_anchor(&mut self, layout: &DocLayout) {
        let Some(block_idx) = self.caret_block() else {
            self.scroll_anchor = None;
            return;
        };
        let Some(block) = layout.blocks.get(block_idx) else {
            return;
        };
        let top = block.bounds.origin.y.as_f32() - layout.origin.y.as_f32();
        if let Some((anchor_block, anchor_top, anchor_scroll)) = self.scroll_anchor
            && anchor_block == block_idx
        {
            let delta = top - anchor_top;
            if delta.abs() > 0.5 {
                self.set_scroll_offset_y((anchor_scroll + delta).max(0.));
            }
        }
    }

    /// Store the scroll anchor *after* any scroll-into-view correction, so the
    /// next frame compares against the offset the user will actually see.
    fn record_scroll_anchor(&mut self, layout: &DocLayout) {
        let Some(block_idx) = self.caret_block() else {
            self.scroll_anchor = None;
            return;
        };
        let Some(block) = layout.blocks.get(block_idx) else {
            self.scroll_anchor = None;
            return;
        };
        let top = block.bounds.origin.y.as_f32() - layout.origin.y.as_f32();
        self.scroll_anchor = Some((block_idx, top, self.scroll_offset_y()));
    }
}

// ---------------------------------------------------------------- frame cost (7.1)

/// What one simulated frame cost, split the way the frame budget is spent: the
/// layout (measuring) pass and the shaping (`prepaint`) pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerfSample {
    pub layout_ms: f64,
    pub shape_ms: f64,
    pub blocks: usize,
    pub fragments: usize,
    /// Fragments actually shaped for painting.
    pub shaped: usize,
    /// Fragments the viewport cull dropped (shaped in no frame at all).
    pub culled: usize,
    pub height: f32,
}

impl PerfSample {
    /// Layout plus shaping: what one frame of "重排" costs.
    pub fn total_ms(&self) -> f64 {
        self.layout_ms + self.shape_ms
    }
}

impl MarkdownView {
    /// The shaped-fragment cache of this view (shared with every frame).
    pub fn shape_cache(&self) -> &Arc<ShapeCache> {
        &self.shape_cache
    }

    /// The per-block geometry cache of this view.
    pub fn block_cache(&self) -> &BlockCache {
        &self.block_cache
    }

    /// The metrics of the last painted frame, if a frame has painted.
    pub fn frame_metrics(&self) -> Option<Arc<WindowMetrics>> {
        self.frame_metrics.clone()
    }

    /// Throw both caches away, so the next frame pays the cold price. This is what
    /// `--dump-perf` uses to report the before/after of task 7.1 honestly.
    pub fn clear_caches(&mut self) {
        self.block_cache.clear();
        self.shape_cache.clear();
    }

    /// The layout this view would produce right now, through the block cache.
    ///
    /// Only the frame path uses this: [`MarkdownView::layout_now`] stays
    /// cache-free so a probe's one-off re-layout cannot be confused with the
    /// steady-state cost.
    pub fn layout_now_cached(
        &mut self,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
    ) -> DocLayout {
        self.layout_windowed_cached(metrics, origin, width, None)
    }

    /// Viewport-first layout: blocks inside `window` are measured now, the rest
    /// keep an estimated height until they scroll in or the background pass
    /// fills the block cache. `None` measures the whole document (headless
    /// callers and streaming-disabled runs).
    pub fn layout_windowed_cached(
        &mut self,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
        window: Option<Viewport>,
    ) -> DocLayout {
        let media = &self.media;
        let generation = media.generation();
        let mut layout = DocLayout::layout_windowed_media(
            &self.doc,
            &self.theme,
            metrics,
            origin,
            width,
            self.projection_mode(),
            self.marked.as_ref(),
            media,
            generation,
            Some(&mut self.block_cache),
            window,
        );
        layout.ensure_caret_line(&self.doc, self.caret);
        layout
    }

    /// Measure every placeholder in `layout` that intersects `viewport`.
    pub fn ensure_window(&mut self, layout: &mut DocLayout, viewport: Viewport) -> usize {
        let generation = self.media.generation();
        layout.ensure_measured(
            &self.doc,
            &self.media,
            generation,
            viewport,
            Some(&mut self.block_cache),
        )
    }

    /// Fill the block cache for blocks outside the measured window.
    pub fn background_measure(
        &mut self,
        layout: &mut DocLayout,
        limit: usize,
        budget_ms: f64,
    ) -> usize {
        let band = layout.window.unwrap_or(Viewport::UNBOUNDED);
        let targets = layout.unmeasured_near(band, limit);
        if targets.is_empty() {
            return 0;
        }
        let generation = self.media.generation();
        layout.measure_background(
            &self.doc,
            &self.media,
            generation,
            &targets,
            &mut self.block_cache,
            budget_ms,
        )
    }

    /// One frame's share of the background pass: measure the nearest blocks
    /// outside the window into the block cache. Returns true while work remains,
    /// so the caller can request another frame.
    pub fn background_step(&mut self, layout: &mut DocLayout) -> bool {
        if !self.streaming_enabled() || layout.estimated_blocks == 0 {
            return false;
        }
        if layout.window.is_none() {
            return false;
        }
        self.background_measure(layout, 24, 4.0) > 0
    }

    /// One simulated frame at a fixed page width, viewport height and scroll
    /// offset: re-lay the document out through the block cache, then shape only
    /// the fragments the viewport shows. Both phases are timed separately, which
    /// is the attribution task 7.1 asks for (layout vs shaping vs text copies).
    ///
    /// It goes through the same two functions the real frame does
    /// ([`MarkdownView::layout_now_cached`] and [`shape_frame`]), so the numbers
    /// are the frame's, not a benchmark of a different code path.
    pub fn perf_frame(
        &mut self,
        metrics: Arc<WindowMetrics>,
        width: Pixels,
        viewport_height: f32,
        scroll: f32,
        cull: bool,
    ) -> PerfSample {
        let started = Instant::now();
        let layout = self.layout_now_cached(metrics.clone(), point(px(0.), px(0.)), width);
        let layout_ms = started.elapsed().as_secs_f64() * 1000.;

        let viewport = if cull {
            Viewport::on_screen(-scroll, viewport_height, viewport_height * CULL_MARGIN)
        } else {
            Viewport::UNBOUNDED
        };
        let started = Instant::now();
        let (texts, quads, shaped, culled) = shape_frame(&metrics, &layout, viewport);
        let shape_ms = started.elapsed().as_secs_f64() * 1000.;
        // The commands are what a real frame would hand to `paint`; keeping them
        // alive past the timer is deliberate, so the optimizer cannot drop the
        // shaping this sample is measuring.
        drop((texts, quads));

        let fragments = layout.blocks.iter().map(|b| b.spans.len()).sum::<usize>();
        let blocks = layout.blocks.len();
        let height = layout.size.height.as_f32();
        self.layout = Some(layout);
        self.stale = false;
        PerfSample {
            layout_ms,
            shape_ms,
            blocks,
            fragments,
            shaped,
            culled,
            height,
        }
    }
}

// ---------------------------------------------------------------- caret

impl MarkdownView {
    /// Move the caret to a source offset (clamped to the buffer, snapped down).
    /// This collapses any selection: a caret with an anchor is a selection, and
    /// setting one end is "put the caret here".
    pub fn set_caret(&mut self, src: usize) {
        let src = self.doc.snap_floor(src);
        self.caret = Some(src);
        self.anchor = Some(src);
        self.desired_x = None;
        self.stale = true;
        self.reset_blink();
    }

    /// Drop the caret: the document goes back to being fully rendered.
    pub fn clear_caret(&mut self) {
        self.caret = None;
        self.anchor = None;
        self.desired_x = None;
        self.stale = true;
    }

    /// Source offset -> `(block, display byte)` in the *current* layout.
    pub fn display_byte(&self) -> Option<(usize, usize)> {
        let caret = self.caret?;
        let layout = self.layout.as_ref()?;
        self.caret_display_byte(layout, caret)
    }

    /// The caret's display position, including the reserved line the caret owns
    /// after the document's final newline: that line has no painted text, so the
    /// byte is one past the block's display text (the sentinel `caret_x` and
    /// `line_tops` recognise).
    pub fn caret_display_byte(&self, layout: &DocLayout, src: usize) -> Option<(usize, usize)> {
        let (block_idx, byte) = layout.display_byte_for_src(&self.doc, src)?;
        let block = layout.blocks.get(block_idx)?;
        if block.extra_caret_line.as_f32() > 0.5
            && self
                .doc
                .blocks()
                .get(block_idx)
                .is_some_and(|source| source.src.end == src)
        {
            return Some((block_idx, block.display_text().len() + 1));
        }
        Some((block_idx, byte))
    }

    pub fn caret_state(&self) -> Option<CaretState> {
        let src = self.caret?;
        let layout = self.layout.as_ref()?;
        let (block_idx, display_byte) = self.caret_display_byte(layout, src)?;
        Some(CaretState {
            src,
            block_idx,
            display_byte,
            bounds: layout.caret_bounds(block_idx, display_byte),
        })
    }

    /// Caret rectangle in window coordinates.
    pub fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        self.caret_state().and_then(|state| state.bounds)
    }

    /// Caret rectangle for an arbitrary source offset, in the geometry the
    /// current layout has (group 6's `bounds_for_range` primitive).
    pub fn bounds_for_src(&self, src: usize) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let (block_idx, display_byte) = if Some(src) == self.caret {
            self.caret_display_byte(layout, src)?
        } else {
            layout.display_byte_for_src(&self.doc, src)?
        };
        layout.caret_bounds(block_idx, display_byte)
    }

    /// Recompute the stored layout from the current buffer through the metrics of
    /// the last painted frame. This is what "重排" means here: a full reparse
    /// (in `Document::apply_edit`) plus a full relayout, which is the whole M0
    /// budget (see findings H1).
    pub fn sync_layout(&mut self) {
        let base = match &self.layout {
            Some(base) => (base.metrics.clone(), base.origin, base.size.width),
            None => return,
        };
        let (metrics, origin, width) = base;
        self.layout = Some(self.layout_now(metrics, origin, width));
        self.stale = false;
    }

    /// The layout this view would produce right now: current projection, current
    /// caret, and the live IME composition spliced in.
    ///
    /// One constructor for every caller (`request_layout`, `paint`, `sync_layout`,
    /// [`MarkdownView::seed_layout`]) so a composition can never be visible in one
    /// of them and missing in another.
    pub fn layout_now(
        &self,
        metrics: Arc<dyn Metrics>,
        origin: Point<Pixels>,
        width: Pixels,
    ) -> DocLayout {
        let media = &self.media;
        let generation = media.generation();
        let mut layout = DocLayout::layout_mode_marked_cached_media(
            &self.doc,
            &self.theme,
            metrics,
            origin,
            width,
            self.projection_mode(),
            self.marked.as_ref(),
            media,
            generation,
            None,
        );
        layout.ensure_caret_line(&self.doc, self.caret);
        layout
    }

    /// Lay the document out with `source_block` projected as source, reusing the
    /// metrics, origin and width of the current layout.
    pub fn layout_for_source(&self, source_block: Option<usize>) -> Option<DocLayout> {
        let base = self.layout.as_ref()?;
        let media = &self.media;
        let generation = media.generation();
        Some(DocLayout::layout_mode_marked_cached_media(
            &self.doc,
            &self.theme,
            base.metrics.clone(),
            base.origin,
            base.size.width,
            LayoutMode { source_block },
            None,
            media,
            generation,
            None,
        ))
    }

    /// Lay the document out without a window, so the headless probe can run the
    /// same caret code. Reports itself in the dump (`measure:` line).
    pub fn seed_layout(&mut self, metrics: Arc<dyn Metrics>, width: Pixels) {
        self.layout = Some(self.layout_now(metrics, point(px(0.), px(0.)), width));
        self.stale = false;
    }

    /// Guarantee a layout exists (window mode paints one; the probe may not have
    /// a surface at all).
    pub fn ensure_layout(&mut self) {
        if self.layout.is_none() {
            self.seed_layout(Arc::new(typors_render::ApproxMetrics), px(1000.));
        }
    }

    /// Window point -> source offset, resolved in the geometry the click will
    /// actually produce.
    ///
    /// The clicked block flips to the source projection, which changes its own
    /// geometry (the `## ` prefix is painted again, and the block can re-wrap).
    /// Resolving the hit against that pending geometry is what makes the caret
    /// land where the user sees it after the switch instead of one frame behind.
    pub fn point_to_src(&self, p: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        if let Some(object) = layout.object_at(p) {
            return Some(if object.right_half {
                object.source.end
            } else {
                object.source.start
            });
        }
        let first = layout.hit(p)?;
        // Only chart/Mermaid fences switch to the source projection when the caret
        // enters them; every other block keeps the geometry it was hit in. Building
        // a source layout for an ordinary code block would measure the click
        // against the hidden fence lines instead of the rendered code lines, which
        // is why only the first and last line used to be reachable.
        if !self.doc.block_is_rich_fence(first.block_idx) {
            return layout.src_for_display_byte(&self.doc, first.block_idx, first.render_byte);
        }
        let pending = self.layout_for_source(Some(first.block_idx))?;
        let hit = pending.hit(p).unwrap_or(first);
        pending.src_for_display_byte(&self.doc, hit.block_idx, hit.render_byte)
    }

    /// Measure the block under a point when it is still a placeholder, so hit
    /// testing works for a click that arrives before the background pass did.
    fn ensure_point_measured(&mut self, p: Point<Pixels>) {
        for _ in 0..3 {
            let Some(layout) = self.layout.as_ref() else {
                return;
            };
            let Some(block_idx) = layout.block_at_y(p.y) else {
                return;
            };
            if layout.blocks.get(block_idx).is_none_or(|block| block.measured) {
                return;
            }
            let Some(mut layout) = self.layout.take() else {
                return;
            };
            let generation = self.media.generation();
            layout.ensure_blocks(
                &self.doc,
                &self.media,
                generation,
                block_idx..=block_idx,
                Some(&mut self.block_cache),
            );
            self.layout = Some(layout);
        }
    }

    /// Measure the caret's block (and up to 2000 blocks above it) when it is still
    /// outside the measured window. Returns true when geometry changed.
    pub fn ensure_caret_measured(&mut self, layout: &mut DocLayout) -> bool {
        let Some(caret) = self.caret else {
            return false;
        };
        let Some(block_idx) = self.doc.block_at_src(caret) else {
            return false;
        };
        if layout
            .blocks
            .get(block_idx)
            .is_none_or(|block| block.measured)
        {
            return false;
        }
        let start = block_idx.saturating_sub(2000);
        let generation = self.media.generation();
        layout.ensure_blocks(
            &self.doc,
            &self.media,
            generation,
            start..=block_idx,
            Some(&mut self.block_cache),
        ) > 0
    }

    /// Point the caret at a window point (a mouse click). The press also sets the
    /// selection anchor, so a following drag (or shift-click) has somewhere to
    /// start from.
    pub fn click(&mut self, p: Point<Pixels>) -> Option<usize> {
        // A click can land on a block the windowed layout has not measured yet
        // (a probe aims below the fold; a real click lands in the visible band,
        // which prepaint measured). Measure it, then hit test for real.
        self.ensure_point_measured(p);
        let src = self.point_to_src(p)?;
        self.caret = Some(src);
        self.anchor = Some(src);
        self.desired_x = None;
        self.stale = true;
        self.reset_blink();
        Some(src)
    }

    /// Apply one keyboard movement. Returns true when the caret moved.
    pub fn move_caret(&mut self, mv: CaretMove) -> bool {
        self.move_caret_with(mv, false)
    }

    /// Apply one keyboard movement with the selection kept: what every `shift`+
    /// arrow does. Returns true when the caret moved.
    pub fn extend_caret(&mut self, mv: CaretMove) -> bool {
        self.move_caret_with(mv, true)
    }

    fn move_caret_with(&mut self, mv: CaretMove, extend: bool) -> bool {
        let Some(caret) = self.caret else {
            // The first movement puts the caret somewhere sensible.
            let target = match mv {
                CaretMove::DocEnd => self.doc.len_bytes(),
                CaretMove::Down | CaretMove::LineEnd => self.doc.len_bytes(),
                _ => 0,
            };
            self.set_caret(target);
            return true;
        };
        let target = match mv {
            CaretMove::Left => prev_char_boundary(&self.doc, caret),
            CaretMove::Right => next_char_boundary(&self.doc, caret),
            CaretMove::DocStart => 0,
            CaretMove::DocEnd => self.doc.len_bytes(),
            CaretMove::LineStart => self.block_start(caret),
            CaretMove::LineEnd => self.block_end(caret),
            CaretMove::Up => {
                let Some(target) = self.vertical_target(caret, -1) else {
                    return false;
                };
                target
            }
            CaretMove::Down => {
                let Some(target) = self.vertical_target(caret, 1) else {
                    return self.jump_block(caret, 1);
                };
                if target == caret {
                    return self.jump_block(caret, 1);
                }
                target
            }
        };
        if matches!(mv, CaretMove::Up | CaretMove::Down) {
            // Keep the x the user started from, so a run of down-arrows follows
            // one column rather than the shortest line in the middle.
            if self.desired_x.is_none() {
                self.desired_x = self.caret_bounds().map(|b| b.origin.x);
            }
        } else {
            self.desired_x = None;
        }
        let moved = target != caret;
        self.caret = Some(self.doc.snap_floor(target));
        // A plain move collapses the selection; an extending move creates one the
        // first time it is asked to keep the old end.
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(caret);
            }
        } else {
            self.anchor = self.caret;
        }
        self.stale = true;
        moved
    }

    fn block_start(&self, src: usize) -> usize {
        self.doc
            .block_at_src(src)
            .and_then(|idx| self.doc.blocks().get(idx).map(|b| b.src.start))
            .unwrap_or(0)
    }

    /// Last-resort vertical move when the target line cannot be hit (a table or
    /// tall object whose paint band overlaps the next block). Still moves one
    /// block, never leaves the caret stuck.
    fn jump_block(&mut self, caret: usize, dir: i32) -> bool {
        let Some(idx) = self.doc.block_at_src(caret) else {
            return false;
        };
        let target = if dir > 0 {
            self.doc.blocks().get(idx + 1).map(|b| b.src.start)
        } else {
            self.doc
                .blocks()
                .get(idx.saturating_sub(1))
                .map(|b| b.src.start)
        };
        let Some(target) = target else {
            return false;
        };
        self.caret = Some(self.doc.snap_floor(target));
        self.anchor = self.caret;
        self.stale = true;
        true
    }

    /// End of the caret block's own line: before the block's trailing newline.
    fn block_end(&self, src: usize) -> usize {
        let Some(block) = self
            .doc
            .block_at_src(src)
            .and_then(|idx| self.doc.blocks().get(idx))
        else {
            return self.doc.len_bytes();
        };
        let text = self.doc.text();
        let slice = text.get(block.src.clone()).unwrap_or_default();
        let trimmed = slice.trim_end_matches('\n').len();
        block.src.start + trimmed
    }

    /// One visual line up (`dir < 0`) or down (`dir > 0`), crossing blocks.
    fn vertical_target(&self, caret: usize, dir: i32) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let (block_idx, display_byte) = layout.display_byte_for_src(&self.doc, caret)?;
        let (x, _top, _line_height) = layout.caret_x(block_idx, display_byte)?;
        let x = self.desired_x.unwrap_or(x);
        let tops = layout.line_tops(block_idx);
        let line = layout.line_index(block_idx, display_byte).unwrap_or(0);

        let same_block = if dir > 0 {
            line + 1 < tops.len()
        } else {
            line > 0
        };
        let target_block = if same_block {
            block_idx
        } else if dir > 0 {
            next_block_with_lines(layout, block_idx)?
        } else {
            prev_block_with_lines(layout, block_idx)?
        };

        // The target's top must be measured in the geometry the move produces,
        // not in the current one. An object block before the target changes
        // height when it leaves the source projection (a 180px image becomes one
        // 26px Markdown line), which would otherwise make the hit-test land on
        // the object that is still rendered.
        let pending_source = self
            .doc
            .block_is_rich_fence(target_block)
            .then_some(target_block);
        let pending = self.layout_for_source(pending_source)?;
        let tops = pending.line_tops(target_block);
        let target_top = if same_block {
            *tops.get((line as i32 + dir) as usize)?
        } else if dir > 0 {
            *tops.first()?
        } else {
            *tops.last()?
        };
        let line_height = pending.blocks.get(target_block)?.line_height;
        let y = px(target_top.as_f32() + line_height.as_f32() * 0.5);
        let hit = pending.hit(point(x, y))?;
        if hit.block_idx != target_block {
            // Tall objects/tables can overlap the next line's hit band. The
            // movement contract is "one visual line", not "whatever paints at
            // this y", so a cross-block move lands at the target's start.
            return Some(pending.blocks.get(target_block)?.src.start);
        }
        pending.src_for_display_byte(&self.doc, hit.block_idx, hit.render_byte)
    }
}

// ---------------------------------------------------------------- selection

impl MarkdownView {
    /// The fixed end of the selection: where the drag started.
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// The live selection, or `None` for a collapsed caret.
    pub fn selection(&self) -> Option<Selection> {
        let (anchor, caret) = (self.anchor?, self.caret?);
        (anchor != caret).then_some(Selection::new(anchor, caret))
    }

    /// The selected source range, in document order.
    pub fn selection_range(&self) -> Option<Range<usize>> {
        self.selection().map(|sel| sel.src_range())
    }

    /// True when the caret is before the anchor (a right-to-left drag).
    pub fn selection_reversed(&self) -> bool {
        self.selection().is_some_and(|sel| sel.is_reversed())
    }

    /// The **source** text of the selection: continuous bytes of the buffer, with
    /// the Markdown markers in them. This is what the clipboard gets, and it is
    /// the same bytes a paste would put back (spec「复制对应的连续源码文本」).
    pub fn selection_text(&self) -> Option<String> {
        self.selection().and_then(|sel| sel.text(&self.doc))
    }

    /// Select `anchor..head`, both source byte offsets.
    pub fn select(&mut self, anchor: usize, head: usize) {
        self.anchor = Some(self.doc.snap_floor(anchor));
        self.caret = Some(self.doc.snap_floor(head));
        self.desired_x = None;
        self.stale = true;
    }

    /// Extend the selection to `head`, keeping the anchor (drag / shift-click).
    pub fn extend_selection_to(&mut self, head: usize) {
        if self.anchor.is_none() {
            self.anchor = self.caret;
        }
        self.caret = Some(self.doc.snap_floor(head));
        self.desired_x = None;
        self.stale = true;
    }

    /// Ctrl+A: the whole buffer.
    pub fn select_all(&mut self) {
        self.select(0, self.doc.len_bytes());
    }

    /// Turn the selection into a collapsed caret (at the caret end).
    pub fn collapse_selection(&mut self) {
        if let Some(caret) = self.caret {
            self.anchor = Some(caret);
        }
        self.stale = true;
    }

    /// Highlight rectangles for the live selection, in the geometry of the last
    /// painted frame (window coordinates).
    pub fn selection_rects(&self) -> Vec<HighlightRect> {
        let (Some(sel), Some(layout)) = (self.selection(), self.layout.as_ref()) else {
            return Vec::new();
        };
        crate::selection::selection_rects(&self.doc, layout, &sel)
    }
}

// ---------------------------------------------------------------- history

impl MarkdownView {
    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo_depth(&self) -> usize {
        self.history.undo_depth()
    }

    pub fn redo_depth(&self) -> usize {
        self.history.redo_depth()
    }

    /// Forget the undo history (a reload; the probe's "start from a clean stack").
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// One undo step: the inverse of the newest record.
    ///
    /// The inverse is [`crate::selection::inverse`]: replace the bytes the record
    /// inserted with the bytes it removed, then put the caret back where
    /// `caret_before` says. It runs through the same single entry point as any
    /// other edit, with [`HistoryAction::Replay`] — that flag is the whole reason
    /// Ctrl+Z followed by Ctrl+Y works: the undo does **not** record itself as a
    /// new edit, it moves its record from the undo stack to the redo stack.
    pub fn undo(&mut self) -> bool {
        let Some(record) = self.history.pop_undo() else {
            return false;
        };
        self.apply_edit_internal(
            undo_range(&record),
            &record.removed,
            HistoryAction::Replay,
            EditTrigger::Undo,
        );
        // `apply_edit` leaves the caret behind the replacement; undo has to put it
        // back where the user left it *before* the edit.
        self.caret = record.caret_before.map(|at| self.doc.snap_floor(at));
        self.anchor = self.caret;
        self.history.push_redo(record);
        true
    }

    /// One redo step: re-apply the newest replayed record. Its record goes back
    /// onto the undo stack and the rest of the redo branch is untouched.
    pub fn redo(&mut self) -> bool {
        let Some(record) = self.history.pop_redo() else {
            return false;
        };
        self.apply_edit_internal(
            record.src.clone(),
            &record.inserted,
            HistoryAction::Replay,
            EditTrigger::Redo,
        );
        self.caret = record.caret_after.map(|at| self.doc.snap_floor(at));
        self.anchor = self.caret;
        self.history.push_undo_again(record);
        true
    }
}

// ---------------------------------------------------------------- clipboard

impl MarkdownView {
    /// The shadow copy of the last copy/cut (see [`MarkdownView::clipboard`]).
    pub fn clipboard_shadow(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    /// Ctrl+C. Returns the text that was put on the clipboard: the continuous
    /// **source** slice of the selection.
    ///
    /// The platform write is fire-and-forget (`App::write_to_clipboard` is
    /// synchronous on Windows), and the local shadow is updated from the same
    /// string, so an in-app paste never depends on a platform read succeeding
    /// (findings.md U2).
    pub fn copy(&mut self, cx: &App) -> Option<String> {
        let text = self.selection_text()?;
        self.clipboard = Some(text.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        Some(text)
    }

    /// Ctrl+X: copy, then delete the selection as one undoable edit.
    pub fn cut(&mut self, cx: &App) -> Option<String> {
        let text = self.copy(cx)?;
        if let Some(range) = self.selection_range() {
            self.apply_edit_internal(range, "", HistoryAction::Record, EditTrigger::Cut);
        }
        Some(text)
    }

    /// Ctrl+V. The platform clipboard wins when it has text; the shadow is the
    /// fallback. Returns whether the buffer changed.
    pub fn paste(&mut self, cx: &App) -> bool {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .filter(|text| !text.is_empty())
            .or_else(|| self.clipboard.clone().filter(|text| !text.is_empty()));
        match text {
            Some(text) => self.paste_text(&text),
            None => false,
        }
    }

    /// Paste literal text at the caret, **replacing the selection** when there is
    /// one — the paste half of the clipboard round trip, and the part a probe can
    /// drive without a platform clipboard.
    pub fn paste_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let range = insertion_range(self.caret, self.selection_range());
        self.apply_edit_internal(range, text, HistoryAction::Record, EditTrigger::Paste);
        true
    }
}

fn prev_char_boundary(doc: &Document, at: usize) -> usize {
    let mut i = at.min(doc.len_bytes()).saturating_sub(1);
    while i > 0 && !doc.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(doc: &Document, at: usize) -> usize {
    let len = doc.len_bytes();
    let mut i = at + 1;
    while i < len && !doc.is_char_boundary(i) {
        i += 1;
    }
    i.min(len)
}

fn next_block_with_lines(layout: &DocLayout, block_idx: usize) -> Option<usize> {
    ((block_idx + 1)..layout.blocks.len()).find(|idx| !layout.line_tops(*idx).is_empty())
}

fn prev_block_with_lines(layout: &DocLayout, block_idx: usize) -> Option<usize> {
    (0..block_idx)
        .rev()
        .find(|idx| !layout.line_tops(*idx).is_empty())
}

// ---------------------------------------------------------------- editing

impl MarkdownView {
    /// **The single entry point for every text change.**
    ///
    /// Group 5's undo/redo and group 6's IME commit both come through here: it is
    /// the only place that touches the buffer, the only place that records an
    /// [`EditRecord`], and the only place that keeps the caret consistent with
    /// the re-parsed block table.
    pub fn apply_edit(&mut self, src: Range<usize>, replace_with: &str) {
        self.apply_edit_internal(src, replace_with, HistoryAction::Record, EditTrigger::Api);
    }

    /// The one implementation of [`MarkdownView::apply_edit`], with the undo
    /// history made explicit.
    ///
    /// `action` is how "撤销自身不得污染撤销栈" is enforced rather than hoped for:
    /// a user edit ([`HistoryAction::Record`]) pushes a record and drops the redo
    /// branch; an undo/redo replay ([`HistoryAction::Replay`]) records nothing,
    /// because its caller already moved the record between the two stacks.
    fn apply_edit_internal(
        &mut self,
        src: Range<usize>,
        replace_with: &str,
        action: HistoryAction,
        trigger: EditTrigger,
    ) {
        let edit_started = std::time::Instant::now();
        // Any buffer change ends a composition: the anchor it recorded no longer
        // describes where the user is typing. Windows never tells us a composition
        // ended (`WM_IME_ENDCOMPOSITION` is unhandled), so the commit and cancel
        // paths clear it themselves and this is the backstop for every other edit.
        self.marked = None;
        self.marked_selection = None;
        // Snapshot before anything moves: the log has to report the selection the
        // *edit* saw, not the collapsed one it leaves behind.
        let selection_at_entry = self.selection_range();
        let normalized = normalize_newlines(replace_with);
        let replace_with = normalized.as_ref();
        let start = self.doc.snap_floor(src.start);
        let end = self.doc.snap_ceil(src.end.max(start));
        // Only the removed bytes are copied. A whole-document `text()` here used
        // to make every keystroke O(document) before the parse even started.
        let length_before = self.doc.len_bytes();
        let removed = self.doc.rope().slice(start..end).to_string();
        let caret_before = self.caret;
        self.doc.apply_edit(start..end, replace_with);
        let after = (start + replace_with.len()).min(self.doc.len_bytes());
        self.caret = Some(self.doc.snap_floor(after));
        // An edit collapses the selection onto the caret: whatever was selected is
        // gone, so an anchor pointing into the deleted bytes would be a lie.
        self.anchor = self.caret;
        self.desired_x = None;
        self.edits += 1;
        let record = EditRecord {
            src: start..end,
            removed,
            inserted: replace_with.to_string(),
            caret_before,
            caret_after: Some(after),
            revision: self.doc.revision(),
        };
        match action {
            HistoryAction::Record => self.history.record(record.clone()),
            HistoryAction::Replay => {}
        }
        self.last_edit = Some(record);
        self.last_trigger = Some(trigger);
        if edit_log_enabled() {
            eprintln!(
                "edit trigger={} src={}..{} insert=\"{}\" buf={}B->{}B reparsed={} blocks took={:.2}ms caret={:?}->{:?} sel={:?} rev={}",
                trigger.as_str(),
                start,
                end,
                escape_insert(replace_with),
                length_before,
                self.doc.len_bytes(),
                self.doc.last_edit_blocks(),
                edit_started.elapsed().as_secs_f64() * 1000.0,
                caret_before,
                self.caret,
                selection_at_entry,
                self.doc.revision(),
            );
        }
        // Window mode re-lays-out in `request_layout` for the next frame; a
        // headless caller (the probe) asks for the new layout explicitly.
        self.stale = true;
    }

    /// Insert text at the caret (what `WM_CHAR`, paste and IME commit all mean).
    /// A live selection is replaced — typing over a selection must never insert
    /// next to it.
    pub fn insert_text(&mut self, text: &str) {
        self.insert_text_tagged(text, EditTrigger::Char);
    }

    /// Insert text at the caret, tagged with the entry point for the log.
    pub fn insert_text_tagged(&mut self, text: &str, trigger: EditTrigger) {
        if self.fence_edit.is_some() {
            self.fence_language_typed(text);
            return;
        }
        let range = insertion_range(self.caret, self.selection_range());
        self.apply_edit_internal(range, text, HistoryAction::Record, trigger);
    }

    /// The Enter key's whole effect, minus `cx.notify()`.
    ///
    /// This is the single thing the `Newline` action handler, the
    /// `--dump-newlines` probe and the regression test all call, so the key
    /// path cannot drift from the tested one. Returns whether the buffer grew.
    pub fn insert_newline(&mut self) -> bool {
        let before = self.doc.len_bytes();
        self.insert_text_tagged("\n", EditTrigger::Newline);
        self.doc.len_bytes() > before
    }

    /// Backspace: remove the selection if there is one, else the char before the
    /// caret, across block boundaries.
    pub fn delete_backward(&mut self) -> bool {
        if self.fence_edit.is_some() {
            return self.fence_language_backspace();
        }
        if let Some(range) = self.selection_range() {
            self.apply_edit_internal(range, "", HistoryAction::Record, EditTrigger::Backspace);
            return true;
        }
        let Some(caret) = self.caret else {
            return false;
        };
        if caret == 0 {
            return false;
        }
        let prev = prev_char_boundary(&self.doc, caret);
        self.apply_edit_internal(
            prev..caret,
            "",
            HistoryAction::Record,
            EditTrigger::Backspace,
        );
        true
    }

    /// Delete: remove the selection if there is one, else the char after the
    /// caret, across block boundaries.
    pub fn delete_forward(&mut self) -> bool {
        if let Some(range) = self.selection_range() {
            self.apply_edit_internal(range, "", HistoryAction::Record, EditTrigger::Delete);
            return true;
        }
        let Some(caret) = self.caret else {
            return false;
        };
        if caret >= self.doc.len_bytes() {
            return false;
        }
        let next = next_char_boundary(&self.doc, caret);
        self.apply_edit_internal(caret..next, "", HistoryAction::Record, EditTrigger::Delete);
        true
    }
}

// ---------------------------------------------------------------- actions

impl MarkdownView {
    fn on_action_move(&mut self, mv: CaretMove, cx: &mut Context<Self>) {
        if self.move_caret(mv) {
            cx.notify();
        }
    }

    fn caret_left(&mut self, _: &CaretLeft, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_move(CaretMove::Left, cx);
    }

    fn caret_right(&mut self, _: &CaretRight, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_move(CaretMove::Right, cx);
    }

    fn caret_up(&mut self, _: &CaretUp, _window: &mut Window, cx: &mut Context<Self>) {
        if self.fence_edit.is_some() {
            self.fence_language_move(-1);
            cx.notify();
            return;
        }
        if self.completion.is_some() {
            self.move_completion(-1);
            cx.notify();
            return;
        }
        self.on_action_move(CaretMove::Up, cx);
    }

    fn caret_down(&mut self, _: &CaretDown, _window: &mut Window, cx: &mut Context<Self>) {
        if self.fence_edit.is_some() {
            self.fence_language_move(1);
            cx.notify();
            return;
        }
        if self.completion.is_some() {
            self.move_completion(1);
            cx.notify();
            return;
        }
        self.on_action_move(CaretMove::Down, cx);
    }

    fn line_start(&mut self, _: &LineStart, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_move(CaretMove::LineStart, cx);
    }

    fn line_end(&mut self, _: &LineEnd, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_move(CaretMove::LineEnd, cx);
    }

    fn doc_start(&mut self, _: &DocStart, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_move(CaretMove::DocStart, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_move(CaretMove::DocEnd, cx);
    }

    fn backspace(&mut self, _: &Backspace, _window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_backward() {
            cx.notify();
        }
    }

    fn on_delete_forward_action(
        &mut self,
        _: &DeleteForward,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.delete_forward() {
            cx.notify();
        }
    }

    fn newline(&mut self, _: &Newline, _window: &mut Window, cx: &mut Context<Self>) {
        if self.fence_edit.is_some() {
            self.fence_language_commit(cx);
            return;
        }
        if self.completion.is_some() {
            self.accept_completion();
            cx.notify();
            return;
        }
        self.insert_newline();
        cx.notify();
    }

    // ---------------------------------------------------------- selection keys

    fn extend_left(&mut self, _: &ExtendLeft, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_extend(CaretMove::Left, cx);
    }

    fn extend_right(&mut self, _: &ExtendRight, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_extend(CaretMove::Right, cx);
    }

    fn extend_up(&mut self, _: &ExtendUp, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_extend(CaretMove::Up, cx);
    }

    fn extend_down(&mut self, _: &ExtendDown, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_extend(CaretMove::Down, cx);
    }

    fn extend_line_start(
        &mut self,
        _: &ExtendLineStart,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_extend(CaretMove::LineStart, cx);
    }

    fn extend_line_end(&mut self, _: &ExtendLineEnd, _window: &mut Window, cx: &mut Context<Self>) {
        self.on_extend(CaretMove::LineEnd, cx);
    }

    fn on_extend(&mut self, mv: CaretMove, cx: &mut Context<Self>) {
        if self.extend_caret(mv) {
            cx.notify();
        }
    }

    fn select_all_text(&mut self, _: &SelectAllText, _window: &mut Window, cx: &mut Context<Self>) {
        self.select_all();
        cx.notify();
    }

    // ---------------------------------------------------------- clipboard keys

    fn copy_selection(&mut self, _: &CopySelection, _window: &mut Window, cx: &mut Context<Self>) {
        self.copy(cx);
    }

    fn cut_selection(&mut self, _: &CutSelection, _window: &mut Window, cx: &mut Context<Self>) {
        if self.cut(cx).is_some() {
            cx.notify();
        }
    }

    fn paste_clipboard(
        &mut self,
        _: &PasteClipboard,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.paste(cx) {
            cx.notify();
        }
    }

    // ---------------------------------------------------------- history keys

    fn undo_edit(&mut self, _: &UndoEdit, _window: &mut Window, cx: &mut Context<Self>) {
        if self.undo() {
            cx.notify();
        }
    }

    fn redo_edit(&mut self, _: &RedoEdit, _window: &mut Window, cx: &mut Context<Self>) {
        if self.redo() {
            cx.notify();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if self.fence_edit.is_some() {
            self.fence_edit = None;
        }
        // The overlay scrollbar owns presses inside its band: it must not move the
        // caret, and dragging it must not start a text selection.
        let window_height = window.viewport_size().height.as_f32();
        let window_width = window.viewport_size().width.as_f32();
        if let Some(bar) = self.scrollbar_bounds(window_height, window_width) {
            let slack = 4.0;
            let x = bar.track.origin.x.as_f32();
            let y = bar.track.origin.y.as_f32();
            let width = bar.track.size.width.as_f32();
            let height = bar.track.size.height.as_f32();
            if event.position.x.as_f32() >= x - slack
                && event.position.x.as_f32() <= x + width + slack
                && event.position.y.as_f32() >= y
                && event.position.y.as_f32() <= y + height
            {
                let thumb_top = bar.thumb.origin.y.as_f32();
                let thumb_height = bar.thumb.size.height.as_f32();
                let grab = if event.position.y.as_f32() >= thumb_top
                    && event.position.y.as_f32() <= thumb_top + thumb_height
                {
                    event.position.y.as_f32() - thumb_top
                } else {
                    thumb_height * 0.5
                };
                self.scrollbar_drag = Some(grab);
                self.scroll_to_scrollbar_thumb(event.position.y.as_f32() - grab, window_height);
                cx.notify();
                return;
            }
        }
        // `event.position` is a window coordinate, and the published layout is
        // already in window coordinates (`paint` translated it), so the point is
        // fed in as-is — adding `element.bounds.origin` again would double it.
        if self.click(event.position).is_some() {
            // The press starts a drag: moves until the release extend the
            // selection from this anchor instead of moving the caret.
            self.dragging = true;
            cx.notify();
        } else if self.doc.len_bytes() == 0 {
            // A blank page has no fragment to hit, but it still owns the caret at
            // offset 0. Without this, the first click on an empty document did
            // nothing and typing had no visible insertion point.
            self.caret = Some(0);
            self.anchor = Some(0);
            self.dragging = false;
            self.desired_x = None;
            cx.notify();
        }
    }

    /// Drag selection (5.1): while the left button is down, every move extends the
    /// selection from the anchor the press set. The endpoint is resolved in the
    /// geometry *that move produces* (`point_to_src` flips the endpoint's block to
    /// source first), so the highlight does not lag a frame behind the pointer.
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(grab) = self.scrollbar_drag {
            let window_height = window.viewport_size().height.as_f32();
            self.scroll_to_scrollbar_thumb(event.position.y.as_f32() - grab, window_height);
            cx.notify();
            return;
        }
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(src) = self.point_to_src(event.position) else {
            return;
        };
        if self.caret == Some(src) {
            return;
        }
        self.extend_selection_to(src);
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.dragging = false;
        self.scrollbar_drag = None;
    }
}

/// The key bindings the editor needs. Registered once by the app.
///
/// Key events are not `Element` events: `Element` has no `dispatch_event`
/// (`element.rs:52-143`), so keyboard input goes through gpui's action mechanism
/// (`actions!` + `KeyBinding` + `key_context` + `on_action`, `elements/div.rs:458`).
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("backspace", Backspace, None),
        KeyBinding::new("delete", DeleteForward, None),
        KeyBinding::new("enter", Newline, None),
        KeyBinding::new("left", CaretLeft, None),
        KeyBinding::new("right", CaretRight, None),
        KeyBinding::new("up", CaretUp, None),
        KeyBinding::new("down", CaretDown, None),
        KeyBinding::new("home", LineStart, None),
        KeyBinding::new("end", LineEnd, None),
        KeyBinding::new("ctrl-home", DocStart, None),
        KeyBinding::new("ctrl-end", DocEnd, None),
        // Shift + arrows grow the selection (5.1). Bound without a key context,
        // like the rest, so they work as soon as the editor has focus.
        KeyBinding::new("shift-left", ExtendLeft, None),
        KeyBinding::new("shift-right", ExtendRight, None),
        KeyBinding::new("shift-up", ExtendUp, None),
        KeyBinding::new("shift-down", ExtendDown, None),
        KeyBinding::new("shift-home", ExtendLineStart, None),
        KeyBinding::new("shift-end", ExtendLineEnd, None),
        KeyBinding::new("ctrl-a", SelectAllText, None),
        // Clipboard and history (5.2 / 5.3). Ctrl+Shift+Z is the usual second
        // redo binding on Windows, so both are registered.
        KeyBinding::new("ctrl-c", CopySelection, None),
        KeyBinding::new("ctrl-x", CutSelection, None),
        KeyBinding::new("ctrl-v", PasteClipboard, None),
        KeyBinding::new("ctrl-z", UndoEdit, None),
        KeyBinding::new("ctrl-y", RedoEdit, None),
        KeyBinding::new("ctrl-shift-z", RedoEdit, None),
    ]
}

// ------------------------------------------------- composite coordinates (IME)

impl MarkdownView {
    /// The buffer with a live IME composition spliced in: the text every UTF-16
    /// offset of [`EntityInputHandler`] refers to while composing (design D4).
    fn composite_text(&self) -> String {
        match self.marked.as_ref().filter(|m| !m.is_empty()) {
            Some(m) => m.splice(&self.doc.text(), 0).0,
            None => self.doc.text(),
        }
    }

    /// Composite byte offset -> buffer byte offset.
    fn composite_to_src(&self, composite: usize) -> usize {
        match self.marked.as_ref().filter(|m| !m.is_empty()) {
            Some(m) => m.to_source(composite),
            None => composite,
        }
    }

    /// Buffer byte offset -> composite byte offset.
    fn src_to_composite(&self, src: usize) -> usize {
        match self.marked.as_ref().filter(|m| !m.is_empty()) {
            Some(m) => m.to_composite(src),
            None => src,
        }
    }
}

// ------------------------------------------------- entity input handler (H2)

/// Character input on Windows does **not** arrive as key events: `WM_CHAR` goes
/// straight to the `InputHandler` (`gpui-pre-windows/events.rs:461-468`), which
/// exists only if `paint` called `window.handle_input`. So this impl is what makes
/// typing work at all.
///
/// # The composition contract (group 6, design D4)
///
/// While a composition is live, **every** method here speaks *composite*
/// coordinates: the buffer with the composition spliced in at
/// [`MarkedText::anchor`]. Nothing is written to the rope until the commit, which
/// is what satisfies「组合期间 SHALL NOT 写入缓冲区」without an undo dance, and it is
/// also why nothing here waits for `unmark_text` — on Windows it is never called
/// (`WM_IME_ENDCOMPOSITION` is unhandled), so the commit and cancel paths clear
/// [`MarkdownView::marked`] themselves.
impl EntityInputHandler for MarkdownView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.composite_text();
        let start = utf16_to_byte(&text, range_utf16.start);
        let end = utf16_to_byte(&text, range_utf16.end);
        adjusted_range.replace(byte_to_utf16(&text, start)..byte_to_utf16(&text, end));
        let out = text.get(start..end).unwrap_or_default().to_string();
        crate::ime::log_ime(
            "text_for_range",
            format_args!(
                "range={range_utf16:?} -> {out:?} (composite {}..{}) {}",
                start,
                end,
                crate::ime::state(self)
            ),
        );
        Some(out)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self.caret?;
        let text = self.composite_text();
        // While composing, the platform wants the IME's own cursor inside the
        // composition (it turns this into the candidate-window anchor,
        // `events.rs:634-645`). Without a composition it wants the real selection:
        // anchor..head in composite coordinates, plus which end is the caret.
        let (range, reversed) = match self.marked.as_ref().filter(|m| !m.is_empty()) {
            Some(m) => {
                let at = byte_to_utf16(&text, m.composite_caret());
                (at..at, false)
            }
            None => match self.selection() {
                Some(sel) => {
                    let anchor = self.src_to_composite(sel.anchor);
                    let head = self.src_to_composite(sel.head);
                    (
                        byte_to_utf16(&text, anchor)..byte_to_utf16(&text, head),
                        sel.is_reversed(),
                    )
                }
                None => {
                    let at = byte_to_utf16(&text, caret);
                    (at..at, false)
                }
            },
        };
        let out = UTF16Selection { range, reversed };
        crate::ime::log_ime(
            "selected_range",
            format_args!(
                "utf16={:?} reversed={reversed} {}",
                out.range,
                crate::ime::state(self)
            ),
        );
        Some(out)
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        // Windows never asks (zero references in `gpui-pre-windows`), but a
        // platform that does gets the composition's range in composite coordinates.
        let m = self.marked.as_ref().filter(|m| !m.is_empty())?;
        let text = self.composite_text();
        let range = m.composite_range();
        Some(byte_to_utf16(&text, range.start)..byte_to_utf16(&text, range.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let had = self.marked.take();
        self.marked_selection = None;
        crate::ime::log_ime(
            "unmark_text",
            format_args!("had={} {}", had.is_some(), crate::ime::state(self)),
        );
        if had.is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Keyboard input arrives here, not through insert_text_tagged. While the
        // fence-language picker is open the characters filter it instead of
        // editing the document.
        if self.fence_edit.is_some() {
            self.fence_language_typed(text);
            cx.notify();
            return;
        }
        let composite = self.composite_text();
        let before = crate::ime::state(self);
        let (range, committed) = match self.marked.as_ref().filter(|m| !m.is_empty()) {
            Some(m) => {
                // The platform commits a composition with `range = None` and the
                // whole result in `text` (`events.rs:728-734`); a range, when it
                // does come, is in composite coordinates and may cover any part of
                // the composition, which collapses onto the anchor.
                let (from, to) = match &range_utf16 {
                    Some(range) => (
                        utf16_to_byte(&composite, range.start),
                        utf16_to_byte(&composite, range.end),
                    ),
                    None => (m.composite_range().start, m.composite_range().end),
                };
                let start = m.to_source(from);
                let end = m.to_source(to).max(start);
                (start..end, true)
            }
            None => match &range_utf16 {
                Some(range) => (
                    utf16_to_byte(&composite, range.start)..utf16_to_byte(&composite, range.end),
                    false,
                ),
                // 选中后提交必须替换整个选区: the platform sends `None` when the
                // keyboard is not composing, so the caret is not the whole story —
                // a selection is.
                None => match self.selection_range() {
                    Some(sel) => (sel, false),
                    None => {
                        let at = self.caret.unwrap_or(0);
                        (at..at, false)
                    }
                },
            },
        };

        // A composition that began on a selection replaces the whole selection on
        // commit: the composition itself collapses onto its anchor, so without
        // this the selected bytes would survive underneath the committed text.
        let replace = match self.marked_selection.clone() {
            Some(sel) if !text.is_empty() => sel,
            _ => range.clone(),
        };

        if range.is_empty() && text.is_empty() {
            // Cancel: an empty composition plus `replace_text_in_range(None, "")`
            // (`events.rs:720-726`) means "throw the composition away". Nothing is
            // inserted and nothing is deleted, so the rope -- and even its
            // revision -- stays exactly as it was, which is what「取消后缓冲区逐字节
            // 一致」asks for. The selection the composition started on comes back,
            // so Esc leaves the user exactly where they were.
            self.marked = None;
            if let Some(sel) = self.marked_selection.take() {
                self.anchor = Some(sel.start);
                self.caret = Some(sel.end);
            }
            self.stale = true;
            crate::ime::log_ime(
                "cancel",
                format_args!(
                    "result=\"\" range={range_utf16:?} was_marked={committed} {before} -> {}",
                    crate::ime::state(self)
                ),
            );
            // This is the one path where a platform event produces *no* buffer
            // change at all, so it is the one path that could look like "the key
            // did nothing". `TYPORS_EDIT_LOG=1` has to say so, otherwise a session
            // where the IME ate the key and a session where the key never arrived
            // look identical.
            if edit_log_enabled() {
                eprintln!(
                    "edit trigger=ime-cancel src=- insert=\"{}\" buf={}B->{}B caret={:?}->{:?} sel={:?} rev={}",
                    crate::view::escape_insert(text),
                    self.doc.len_bytes(),
                    self.doc.len_bytes(),
                    self.caret,
                    self.caret,
                    self.selection_range(),
                    self.doc.revision(),
                );
            }
            window.invalidate_character_coordinates();
            cx.notify();
            return;
        }

        self.apply_edit_internal(
            replace.clone(),
            text,
            HistoryAction::Record,
            if committed {
                EditTrigger::ImeCommit
            } else {
                EditTrigger::Char
            },
        );
        crate::ime::log_ime(
            "commit",
            format_args!(
                "result={text:?} range={range_utf16:?} was_marked={committed} src={replace:?} {before} -> {}",
                crate::ime::state(self)
            ),
        );
        // The candidate window is anchored on the caret we just moved.
        window.invalidate_character_coordinates();
        self.schedule_completion(cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let composite = self.composite_text();
        let before = crate::ime::state(self);
        // The anchor never moves during one composition -- the rope does not move
        // either -- so an update keeps the anchor the composition started with.
        let selection = self.selection();
        let anchor = match self.marked.as_ref() {
            Some(m) => m.anchor,
            None => {
                let at = match &range_utf16 {
                    Some(range) => self.composite_to_src(utf16_to_byte(&composite, range.start)),
                    // Windows always passes `None` (`events.rs:735-754`). A
                    // composition started on a selection starts at its beginning:
                    // the selection is remembered, not yet deleted.
                    None => selection
                        .map_or_else(|| self.caret.unwrap_or(0), |sel| sel.src_range().start),
                };
                // 选中后提交必须替换整个选区 (and Esc must give it back): from here on
                // the platform only ever tells us about the collapsed anchor, so
                // the range the commit has to replace is recorded now.
                self.marked_selection = selection.map(|sel| sel.src_range());
                self.doc.snap_floor(at)
            }
        };
        let ime_selection = new_selected_range_utf16
            .as_ref()
            .map(|r| utf16_to_byte(new_text, r.start)..utf16_to_byte(new_text, r.end));
        self.marked =
            (!new_text.is_empty()).then(|| MarkedText::new(new_text, anchor, ime_selection));
        // The rope keeps the caret where the composition starts: that is what
        // keeps the block in the source projection for the whole composition
        // (「组合期间源态保持」) and the geometry stable frame to frame.
        self.caret = Some(anchor);
        self.anchor = self.caret;
        self.desired_x = None;
        self.stale = true;
        self.dismiss_completion();
        crate::ime::log_ime(
            "mark",
            format_args!(
                "compstr={new_text:?} sel={new_selected_range_utf16:?} range={range_utf16:?} anchor={anchor} {before} -> {}",
                crate::ime::state(self)
            ),
        );
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // `element_bounds` is deliberately unused. The published layout is already
        // in window coordinates and the platform compares this rectangle against
        // the window (`events.rs:634-681`), so adding the element origin back would
        // shift the candidate window by one element offset.
        let text = self.composite_text();
        let start = utf16_to_byte(&text, range_utf16.start);
        let end = utf16_to_byte(&text, range_utf16.end);
        let layout = self.layout.as_ref()?;
        let start_bounds = layout.caret_bounds_composite(&self.doc, start)?;
        // Same visual line: span from the start's left edge to the end's right
        // edge, keeping the line box's **full height**. The height is not
        // cosmetic: `ImmSetCandidateWindow` anchors the candidate window at the
        // middle of this rectangle (`events.rs:641-642`, `y = origin.y + height/2`),
        // so a zero-height rectangle puts the window half a line too high.
        let out = match layout.caret_bounds_composite(&self.doc, end) {
            Some(end_bounds)
                if (end_bounds.origin.y.as_f32() - start_bounds.origin.y.as_f32()).abs() < 0.5 =>
            {
                let left = start_bounds.origin.x.as_f32();
                let right = end_bounds.origin.x.as_f32() + end_bounds.size.width.as_f32();
                Bounds::new(
                    start_bounds.origin,
                    size(px((right - left).max(2.)), start_bounds.size.height),
                )
            }
            _ => start_bounds,
        };
        crate::ime::log_ime(
            "bounds",
            format_args!(
                "range={range_utf16:?} composite={start}..{end} -> ({:.1},{:.1} {:.1}x{:.1})",
                out.origin.x.as_f32(),
                out.origin.y.as_f32(),
                out.size.width.as_f32(),
                out.size.height.as_f32()
            ),
        );
        Some(out)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let src = self.point_to_src(point)?;
        let composite = self.src_to_composite(src);
        Some(byte_to_utf16(&self.composite_text(), composite))
    }
}

impl Focusable for MarkdownView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page_width = self.page_width_for(window.viewport_size().width.as_f32());
        if !self.blink_started {
            self.blink_started = true;
            let weak = cx.entity().downgrade();
            cx.spawn(async move |_entity, cx| {
                loop {
                    let interval = match weak.update(cx, |view, _| view.blink_interval_ms) {
                        Ok(ms) => ms,
                        Err(_) => break,
                    };
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(interval))
                        .await;
                    let toggled = weak.update(cx, |view, cx| {
                        if view.blink_enabled {
                            view.caret_visible = !view.caret_visible;
                            cx.notify();
                        }
                    });
                    if toggled.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        // Debug hook for task 7.1 only; a no-op unless `TYPORS_SCROLL` is set.
        if !self.scroll_applied
            && let Some(target) = env_f32("TYPORS_SCROLL")
        {
            self.scroll_handle.set_offset(point(px(0.), px(-target)));
            self.scroll_applied = true;
        }
        let focus_handle = self.focus_handle.clone();
        let page = div()
            .id("typors-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .bg(self.theme.page_bg)
            .key_context("TyporsEditor")
            .track_focus(&focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::on_delete_forward_action))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::caret_left))
            .on_action(cx.listener(Self::caret_right))
            .on_action(cx.listener(Self::caret_up))
            .on_action(cx.listener(Self::caret_down))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::extend_left))
            .on_action(cx.listener(Self::extend_right))
            .on_action(cx.listener(Self::extend_up))
            .on_action(cx.listener(Self::extend_down))
            .on_action(cx.listener(Self::extend_line_start))
            .on_action(cx.listener(Self::extend_line_end))
            .on_action(cx.listener(Self::select_all_text))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::cut_selection))
            .on_action(cx.listener(Self::paste_clipboard))
            .on_action(cx.listener(Self::undo_edit))
            .on_action(cx.listener(Self::redo_edit))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            // The scroll container is the whole editor area, so the overlay
            // scrollbar can sit on the window's right edge; the page itself stays
            // centered at its max width.
            .child(
                div()
                    .max_w(px(page_width))
                    .mx_auto()
                    .child(DocElement { view: cx.entity() }),
            );

        if self.show_fps {
            // The real `gpui-fps` HUD (task 7.1). `--dump-perf` cannot host it
            // (nothing is on screen and it exposes no readable statistics), so the
            // probe reads the same frame trace it renders from instead; this is
            // where the crate itself is used the way its README documents.
            return div()
                .relative()
                .size_full()
                .child(page)
                .child(gpui_fps::fps_monitor(window, cx))
                .into_any_element();
        }
        page.into_any_element()
    }
}

// ---------------------------------------------------------------- element

struct DocElement {
    view: Entity<MarkdownView>,
}

/// One resolved paint command, in element-local coordinates.
struct TextCmd {
    origin: Point<Pixels>,
    line_height: Pixels,
    line: ShapedLine,
}

struct QuadCmd {
    bounds: Bounds<Pixels>,
    color: Rgba,
}

/// One successfully resolved image/SVG to paint. Missing content becomes a
/// `TextCmd` placeholder instead, so a failed load still has geometry.
enum ObjectCmd {
    Image {
        bounds: Bounds<Pixels>,
        image: Arc<RenderImage>,
    },
    Svg {
        bounds: Bounds<Pixels>,
        svg: Arc<str>,
    },
}

/// Shape (and select the quads of) the part of `layout` the viewport shows.
///
/// This is the whole of task 7.1's culling. `layout` is always the complete
/// document geometry — `DocLayout::blocks` is indexed by `block_idx`, and hit
/// testing, `caret_x`, `line_tops`, `selection::display_ranges`,
/// `highlight_rects` and every probe read it — so the cull sits here, between the
/// layout and the paint call: a culled frame paints the same pixels and simply
/// does not shape or emit what is off screen.
///
/// Because it is a paint-time filter, the selection's highlight rectangles and
/// the `seam_gaps` continuity check keep running on the **uncropped** geometry
/// (`crate::selection`), which is what keeps a partially scrolled selection from
/// developing a false seam.
///
/// Returns the commands plus `(shaped, culled)` fragment counts.
fn shape_frame(
    metrics: &WindowMetrics,
    layout: &DocLayout,
    viewport: Viewport,
) -> (Vec<TextCmd>, Vec<QuadCmd>, usize, usize) {
    let mut quads = Vec::new();
    let mut texts = Vec::new();
    let mut shaped = 0usize;
    let mut culled = 0usize;
    for block in &layout.blocks {
        let block_top = block.bounds.origin.y.as_f32();
        if !viewport.intersects(block_top, block_top + block.bounds.size.height.as_f32()) {
            culled += block.spans.len();
            continue;
        }
        for decor in &block.decors {
            match decor {
                typors_render::BlockDecor::Quad { bounds, color } => {
                    let top = bounds.origin.y.as_f32();
                    if viewport.intersects(top, top + bounds.size.height.as_f32()) {
                        quads.push(QuadCmd {
                            bounds: *bounds,
                            color: *color,
                        });
                    }
                }
                typors_render::BlockDecor::Text {
                    text,
                    origin,
                    font_size,
                    line_height,
                    color,
                } => {
                    let top = origin.y.as_f32();
                    if viewport.intersects(top, top + line_height.as_f32()) {
                        texts.push(TextCmd {
                            origin: *origin,
                            line_height: *line_height,
                            line: metrics.shaped(
                                text,
                                *font_size,
                                typors_core::doc::Style::PLAIN,
                                false,
                                *color,
                                false,
                            ),
                        });
                    }
                }
            }
        }
        for span in &block.spans {
            let top = span.origin.y.as_f32();
            if !viewport.intersects(top, top + span.line_height.as_f32()) {
                culled += 1;
                continue;
            }
            shaped += 1;
            if span.strike {
                quads.push(QuadCmd {
                    bounds: Bounds::new(
                        point(span.origin.x, px(top + span.line_height.as_f32() * 0.55)),
                        size(span.width, px(1.)),
                    ),
                    color: span.color,
                });
            }
            texts.push(TextCmd {
                origin: span.origin,
                line_height: span.line_height,
                line: metrics.shaped(
                    &span.text,
                    span.font_size,
                    span.style,
                    span.force_bold,
                    span.color,
                    span.underline,
                ),
            });
        }
    }
    (texts, quads, shaped, culled)
}

/// Resolve the visible atomic objects to paint commands. Images subscribe through
/// `use_asset` here (prepaint is inside a frame); a missing/loading image becomes
/// a text placeholder, while math that fails to typeset falls back to its source.
fn collect_objects(
    view: &MarkdownView,
    layout: &DocLayout,
    viewport: Viewport,
    metrics: &WindowMetrics,
    window: &mut Window,
    cx: &mut App,
) -> (Vec<ObjectCmd>, Vec<TextCmd>) {
    let media = view.media();
    let doc_dir = media.doc_dir().map(|path| path.to_path_buf());
    let mut objects = Vec::new();
    let mut texts = Vec::new();
    for block in &layout.blocks {
        for object in &block.objects {
            let top = object.bounds.top().as_f32();
            if !viewport.intersects(top, top + object.bounds.size.height.as_f32()) {
                continue;
            }
            match &object.kind {
                typors_core::doc::ObjectKind::Image {
                    destination, alt, ..
                } => {
                    if let Some(image) = paint_image(destination, doc_dir.as_deref(), window, cx) {
                        objects.push(ObjectCmd::Image {
                            bounds: object.bounds,
                            image,
                        });
                    } else {
                        let label = match media.image_state(destination) {
                            crate::media::ImageState::Pending => {
                                format!("{alt}（图片加载中…）")
                            }
                            crate::media::ImageState::Failed(reason) => {
                                format!("{alt}（图片无法显示：{reason}）")
                            }
                            crate::media::ImageState::Ready { .. } => alt.clone(),
                        };
                        let line = metrics.shaped(
                            &label,
                            object.font_size,
                            typors_core::Style::PLAIN,
                            false,
                            view.theme().muted,
                            false,
                        );
                        texts.push(TextCmd {
                            origin: point(
                                object.bounds.left() + px(4.),
                                object.bounds.top() + px(2.),
                            ),
                            line_height: object.bounds.size.height,
                            line,
                        });
                    }
                }
                typors_core::doc::ObjectKind::Math { tex, .. } => {
                    match media.math().svg(tex, object.font_size.as_f32()) {
                        Ok(svg) => objects.push(ObjectCmd::Svg {
                            bounds: object.bounds,
                            svg,
                        }),
                        Err(error) => {
                            let label = format!("${tex}$  ({error})");
                            let line = metrics.shaped(
                                &label,
                                object.font_size,
                                typors_core::Style::PLAIN,
                                false,
                                rgba(0xCF222EFF),
                                false,
                            );
                            texts.push(TextCmd {
                                origin: point(
                                    object.bounds.left() + px(2.),
                                    object.bounds.top() + px(2.),
                                ),
                                line_height: object.bounds.size.height,
                                line,
                            });
                        }
                    }
                }
                typors_core::doc::ObjectKind::Diagram { source } => {
                    let scale = window.scale_factor().max(0.5);
                    let width = (object.bounds.size.width.as_f32() * scale).round() as i32;
                    let height = (object.bounds.size.height.as_f32() * scale).round() as i32;
                    let renderer = cx.svg_renderer();
                    if let Some(image) = media.diagram_image(source, width, height, &renderer) {
                        objects.push(ObjectCmd::Image {
                            bounds: object.bounds,
                            image,
                        });
                    } else {
                        match media.diagram(source) {
                            Ok(diagram) => objects.push(ObjectCmd::Svg {
                                bounds: object.bounds,
                                svg: diagram.svg.clone(),
                            }),
                            Err(error) => {
                                let label = format!("mermaid 渲染失败：{error}");
                                let line = metrics.shaped(
                                    &label,
                                    object.font_size,
                                    typors_core::Style::PLAIN,
                                    false,
                                    rgba(0xCF222EFF),
                                    false,
                                );
                                texts.push(TextCmd {
                                    origin: point(
                                        object.bounds.left() + px(2.),
                                        object.bounds.top() + px(2.),
                                    ),
                                    line_height: object.bounds.size.height,
                                    line,
                                });
                            }
                        }
                    }
                }
                typors_core::doc::ObjectKind::Chart { source } => {
                    let scale = window.scale_factor().max(0.5);
                    let width = (object.bounds.size.width.as_f32() * scale).round() as i32;
                    let height = (object.bounds.size.height.as_f32() * scale).round() as i32;
                    let renderer = cx.svg_renderer();
                    if let Some(image) = media.chart_image(source, width, height, &renderer) {
                        objects.push(ObjectCmd::Image {
                            bounds: object.bounds,
                            image,
                        });
                    } else if let Ok(chart) = media.chart(source) {
                        objects.push(ObjectCmd::Svg {
                            bounds: object.bounds,
                            svg: std::sync::Arc::from(chart.svg.as_str()),
                        });
                    } else {
                        let line = metrics.shaped(
                            "chart 渲染失败",
                            object.font_size,
                            typors_core::Style::PLAIN,
                            false,
                            rgba(0xCF222EFF),
                            false,
                        );
                        texts.push(TextCmd {
                            origin: point(
                                object.bounds.left() + px(2.),
                                object.bounds.top() + px(2.),
                            ),
                            line_height: object.bounds.size.height,
                            line,
                        });
                    }
                }
            }
        }
    }
    (objects, texts)
}

impl IntoElement for DocElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DocElement {
    /// The measured layout is computed once per frame in `request_layout` and
    /// handed down; there is no separate `measure` hook on `Element` (the
    /// "intrinsic size" path is `Window::request_measured_layout`, which we do
    /// not need because we lay the block out ourselves).
    type RequestLayoutState = DocLayout;
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("typors-document".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let frame_started = std::time::Instant::now();
        let width = px(window.viewport_size().width.as_f32().min(
            self.view
                .read(cx)
                .page_width_for(window.viewport_size().width.as_f32()),
        ));
        let viewport_height = self
            .view
            .read(cx)
            .viewport_rect(window.viewport_size().height.as_f32())
            .height;
        let view = self.view.clone();
        let (mono_family, cache) = {
            let view = self.view.read(cx);
            (view.theme.mono_family.clone(), view.shape_cache.clone())
        };
        // One `WindowMetrics` for the whole frame: `prepaint` picks it up again
        // below, so measuring here and shaping there share one shape cache
        // (task 7.1, optimisation ①) instead of filling two.
        let metrics = Arc::new(WindowMetrics::with_cache(window, mono_family, cache));
        let (layout, correction) = view.update(cx, |view, cx| {
            // A progressive open publishes its remaining blocks here, on the UI
            // thread; while the parse is pending, keep frames coming so it lands.
            view.poll_progressive_parse();
            if view.parse_pending() {
                cx.notify();
            }
            view.refresh_media(window, cx);
            view.frame_metrics = Some(metrics.clone());
            // Viewport-first: only the band the frame can show is measured now.
            // The band is derived from the scroll offset, which the scroll
            // container has already applied by the time this pass runs.
            let scroll = view.scroll_offset_y();
            let streaming = view.streaming_enabled();
            let band = streaming.then(|| {
                Viewport::on_screen(
                    -scroll,
                    viewport_height,
                    viewport_height * CULL_MARGIN,
                )
            });
            let mut layout =
                view.layout_windowed_cached(metrics, point(px(0.), px(0.)), width, band);
            // A caret parked outside the window (Ctrl+End, a click below the fold)
            // needs the blocks between it and the window measured before the scroll
            // decision, or the correction would use estimated positions.
            view.ensure_caret_measured(&mut layout);
            if let (Some(caret), true) = (view.caret, band.is_some())
                && let Some(block_idx) = view.doc.block_at_src(caret)
                && layout
                    .blocks
                    .get(block_idx)
                    .is_some_and(|block| !block.measured)
            {
                // The 2000-block cap left work for the next frame.
                cx.notify();
            }
            // Caret scroll-into-view ("回车后光标不跟随"). It has to run here, in the
            // layout pass, for two reasons:
            //
            // 1. This is the first moment a layout of the *new* buffer exists. The
            //    published layout is deliberately one frame behind (`stale`), so a
            //    handler that ran straight after an edit has no geometry to check.
            // 2. The scroll container reads its offset in its own `prepaint`, which
            //    gpui runs **after** every `request_layout`. Writing the offset here
            //    therefore lands in this very frame -- no extra frame, no flicker.
            view.apply_scroll_anchor(&layout);
            let correction = view.scroll_for_caret_if_moved(&layout, viewport_height);
            // The caret correction can move the viewport after the layout was
            // measured; measure the destination band in the same frame so the
            // corrected frame never paints a placeholder.
            if let Some(correction) = &correction
                && band.is_some()
            {
                let target = Viewport::on_screen(
                    -correction.to,
                    viewport_height,
                    viewport_height * CULL_MARGIN,
                );
                let generation = view.media.generation();
                layout.ensure_measured(
                    &view.doc,
                    &view.media,
                    generation,
                    target,
                    Some(&mut view.block_cache),
                );
            }
            view.record_scroll_anchor(&layout);
            // Spend a slice of this frame on the nearest unmeasured blocks and
            // ask for another frame while any remain.
            if view.background_step(&mut layout) {
                cx.notify();
            }
            (layout, correction)
        });
        if let Some(correction) = correction
            && view.read(cx).cull_log
        {
            eprintln!(
                "scroll: caret {:.1}..{:.1} -> offset {:.1} -> {:.1} ({:+.1}), viewport {viewport_height:.1}",
                correction.caret_top,
                correction.caret_bottom,
                correction.from,
                correction.to,
                correction.moved(),
            );
        }

        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = layout.size.height.into();
        if std::env::var("TYPORS_FRAME_LOG").is_ok() {
            eprintln!(
                "frame request_layout took {:.2} ms (page {:.1})",
                frame_started.elapsed().as_secs_f64() * 1000.0,
                layout.size.height.as_f32()
            );
        }
        (window.request_layout(style, [], cx), layout)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let started = std::time::Instant::now();
        let prepaint_started = started;
        let metrics = match self.view.read(cx).frame_metrics.clone() {
            Some(metrics) => metrics,
            None => {
                let mono_family = request_layout.theme.mono_family.clone();
                Arc::new(WindowMetrics::new(window, mono_family))
            }
        };
        // Cull to what this frame can actually show. `bounds.origin` is in window
        // coordinates and the scroll container has already moved the element, so
        // the visible band in element-local coordinates starts at
        // `-bounds.origin.y`.
        // The scroll container is smaller than the window (menu and status bars);
        // the shell publishes its band so culling and caret scrolling agree with
        // what is actually visible.
        let viewport_rect = self
            .view
            .read(cx)
            .viewport_rect(window.viewport_size().height.as_f32());
        let viewport_height = viewport_rect.height;
        let band_top = viewport_rect.top - bounds.origin.y.as_f32();
        let viewport = if band_top.is_finite() && viewport_height > 0. {
            let margin = viewport_height * CULL_MARGIN;
            Viewport::new(band_top - margin, band_top + viewport_height + margin)
        } else {
            Viewport::UNBOUNDED
        };
        // The paint band is the authority: whatever this frame can show must be
        // measured before it is shaped. Normally the layout pass already did it;
        // this covers a scroll that landed after the layout pass (the container
        // clamps the offset in its own prepaint).
        let late = self.view.update(cx, |view, _cx| {
            let generation = view.media.generation();
            request_layout.ensure_measured(
                &view.doc,
                &view.media,
                generation,
                viewport,
                Some(&mut view.block_cache),
            )
        });
        if late > 0 && self.view.read(cx).cull_log {
            eprintln!("cull: measured {late} late blocks for the paint band");
        }
        let (mut texts, mut quads, shaped, culled) =
            shape_frame(&metrics, request_layout, viewport);
        // Overlay scrollbar on the page's right edge, only when it overflows.
        let scrollbar = {
            let view = self.view.read(cx);
            let rect = view.viewport_rect(window.viewport_size().height.as_f32());
            let scroll = rect.top - bounds.origin.y.as_f32();
            // The element is the centered page; the bar lives on the window's
            // right edge, so convert that edge into element-local coordinates.
            let window_right =
                window.viewport_size().width.as_f32() - bounds.origin.x.as_f32();
            typors_render::scrollbar_geometry(
                window_right,
                request_layout.size.height.as_f32(),
                scroll,
                rect.height,
                scroll,
            )
        };
        if let Some(bar) = scrollbar {
            quads.push(QuadCmd {
                bounds: bar.track,
                color: request_layout.theme.scrollbar_track,
            });
            quads.push(QuadCmd {
                bounds: bar.thumb,
                color: request_layout.theme.scrollbar_thumb,
            });
        }
        let (objects, object_texts) = self.view.update(cx, |view, cx| {
            collect_objects(view, request_layout, viewport, metrics.as_ref(), window, cx)
        });
        texts.extend(object_texts);
        request_layout.shape_ms = started.elapsed().as_secs_f64() * 1000.;
        request_layout.shaped_fragments = shaped;
        request_layout.culled_fragments = culled;
        if self.view.read(cx).cull_log {
            eprintln!(
                "cull: element_top {:.1}, viewport {:.1}, band {:.1}..{:.1}, shaped {shaped}, culled {culled}",
                bounds.origin.y.as_f32(),
                viewport_height,
                viewport.top,
                viewport.bottom,
            );
        }

        // The caret is a quad in element-local coordinates, shifted with
        // everything else in `paint`.
        let caret = {
            let view = self.view.read(cx);
            if !view.caret_visible {
                None
            } else {
                view.caret_display_position(request_layout)
                    .and_then(|(block_idx, display_byte)| {
                        request_layout
                            .caret_bounds(block_idx, display_byte)
                            .map(|bounds| QuadCmd {
                                bounds,
                                color: request_layout.theme.caret,
                            })
                    })
            }
        };

        // Selection highlight (5.1): one quad per painted fragment the selection
        // covers, in the same element-local coordinates as the text, so `paint`
        // shifts both by the same origin. Drawn *under* the glyphs.
        //
        // The rectangles come from the *uncropped* layout (so the geometry and the
        // `seam_gaps` continuity check are unaffected by culling); only the quads
        // that cannot be on screen are dropped here, so a screen-out selection
        // never turns into paint commands.
        let highlight = {
            let view = self.view.read(cx);
            match view.selection() {
                Some(sel) => crate::selection::selection_rects(&view.doc, request_layout, &sel)
                    .into_iter()
                    .filter(|rect| {
                        viewport.intersects(rect.line_top, rect.line_top + rect.line_height)
                    })
                    .map(|rect| QuadCmd {
                        bounds: rect.bounds(),
                        color: request_layout.theme.selection,
                    })
                    .collect(),
                None => Vec::new(),
            }
        };

        if std::env::var("TYPORS_FRAME_LOG").is_ok() {
            eprintln!(
                "frame prepaint took {:.2} ms shaped {shaped} culled {culled}",
                prepaint_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        PrepaintState {
            highlight,
            quads,
            objects,
            texts,
            caret,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();

        // The only legal place for this call (`window.rs:5395` asserts we are in
        // paint), and the channel every character and IME event arrives on. It
        // is a no-op while the element is not focused.
        if focus_handle.is_focused(window) {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
        }

        let dx = bounds.origin.x;
        let dy = bounds.origin.y;
        let shift = |p: Point<Pixels>| {
            point(
                px(p.x.as_f32() + dx.as_f32()),
                px(p.y.as_f32() + dy.as_f32()),
            )
        };

        for quad in &prepaint.highlight {
            let mut box_ = quad.bounds;
            box_.origin = shift(box_.origin);
            window.paint_quad(fill(box_, quad.color));
        }
        for quad in &prepaint.quads {
            let mut box_ = quad.bounds;
            box_.origin = shift(box_.origin);
            window.paint_quad(fill(box_, quad.color));
        }
        for object in &prepaint.objects {
            match object {
                ObjectCmd::Image {
                    bounds: object_bounds,
                    image,
                } => {
                    let mut box_ = *object_bounds;
                    box_.origin = shift(box_.origin);
                    let painted =
                        window.paint_image(box_, box_, Corners::default(), image.clone(), 0, false);
                    debug_assert!(painted.is_ok(), "painting an image failed: {painted:?}");
                }
                ObjectCmd::Svg {
                    bounds: object_bounds,
                    svg,
                } => {
                    let mut box_ = *object_bounds;
                    box_.origin = shift(box_.origin);
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    svg.hash(&mut hasher);
                    (box_.size.width.as_f32()).to_bits().hash(&mut hasher);
                    (box_.size.height.as_f32()).to_bits().hash(&mut hasher);
                    let key: SharedString = format!("typors-math-{:016x}", hasher.finish()).into();
                    let painted = window.paint_svg(
                        box_,
                        key,
                        Some(svg.as_bytes()),
                        TransformationMatrix::default(),
                        request_layout.theme.text.into(),
                        cx,
                    );
                    debug_assert!(painted.is_ok(), "painting math failed: {painted:?}");
                }
            }
        }
        for cmd in &prepaint.texts {
            let painted = cmd.line.paint(
                shift(cmd.origin),
                cmd.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
            debug_assert!(
                painted.is_ok(),
                "painting a shaped line failed: {painted:?}"
            );
        }
        if focus_handle.is_focused(window)
            && let Some(caret) = &prepaint.caret
        {
            let mut box_ = caret.bounds;
            box_.origin = shift(box_.origin);
            window.paint_quad(fill(box_, caret.color));
        }

        // Publish the finished layout in window coordinates: this is the object
        // `--dump-layout` prints and the one hit testing uses.
        let mut layout = request_layout.clone();
        layout.translate(bounds.origin);

        let sink = self.view.read(cx).dump.clone();
        if let Some(sink) = sink {
            let already = sink.lock().map(|guard| guard.is_some()).unwrap_or(true);
            if !already {
                let report = layout.report();
                let checks = {
                    let view = self.view.read(cx);
                    layout.checks(&view.doc)
                };
                // Second pass with the same text system, already warm: the
                // difference against `layout.measure_ms` is font-system warm-up,
                // which matters for H1 (cold frame vs steady state).
                let warm = std::time::Instant::now();
                {
                    let view = self.view.read(cx);
                    let metrics =
                        Arc::new(WindowMetrics::new(window, view.theme.mono_family.clone()));
                    let _ = view.layout_now(metrics, point(px(0.), px(0.)), bounds.size.width);
                }
                let remeasure_ms = warm.elapsed().as_secs_f64() * 1000.;
                // ... and a third pass through task 7.1's caches (block geometry +
                // shaped fragments). The pair `remeasure_ms` / `cached_ms` is the
                // before/after of the optimisation, from one run, on one document.
                let mono_family = self.view.read(cx).theme.mono_family.clone();
                let shape_cache = self.view.read(cx).shape_cache.clone();
                let cached_started = std::time::Instant::now();
                self.view.update(cx, |view, _cx| {
                    let metrics =
                        Arc::new(WindowMetrics::with_cache(window, mono_family, shape_cache));
                    let _ =
                        view.layout_now_cached(metrics, point(px(0.), px(0.)), bounds.size.width);
                });
                let cached_ms = cached_started.elapsed().as_secs_f64() * 1000.;
                if let Ok(mut guard) = sink.lock() {
                    *guard = Some(DumpCapture {
                        layout: layout.clone(),
                        report,
                        checks,
                        remeasure_ms,
                        cached_ms,
                    });
                }
            }
        }
        let painted_objects = prepaint.objects.len();
        self.view.update(cx, |view, _cx| {
            view.layout = Some(layout);
            view.stale = false;
            view.painted_objects = painted_objects;
        });

        // `--dump-media`: the probe reads the layout and image/math state this
        // frame just produced.
        let (media_sink, media_runner) = {
            let view = self.view.read(cx);
            (view.media_probe.clone(), view.media_runner)
        };
        if let Some(sink) = media_sink
            && let Some(runner) = media_runner
        {
            // Overwrite every frame: a local image starts `Pending` on the first
            // frame and becomes `Ready` a frame or two later, and the dump should
            // report the settled state rather than the first placeholder.
            let capture = self.view.update(cx, |view, _cx| runner(view));
            if let Ok(mut guard) = sink.lock() {
                *guard = Some(capture);
            }
        }

        // `--dump-ime` (group 6): the IME callbacks take a `&mut Window`, which
        // only exists inside a frame, so the probe runs here -- against the layout
        // this frame just painted, i.e. the geometry a user would see.
        let ime = self.view.read(cx).ime_probe.clone();
        if let Some(sink) = ime {
            let already = sink.lock().map(|guard| guard.is_some()).unwrap_or(true);
            if !already {
                let capture = self
                    .view
                    .update(cx, |view, cx| crate::ime::ime_probe(view, window, cx));
                if let Ok(mut guard) = sink.lock() {
                    *guard = Some(capture);
                }
            }
        }

        // `--dump-perf` (task 7.1): the sweep runs here for the same reason the
        // other probes do — it measures with the window text system, and a
        // `Window` only exists inside a frame. It runs *last*, against the layout
        // this frame published, so `view.layout()` is already valid.
        let (perf, perf_runner) = {
            let view = self.view.read(cx);
            (view.perf_probe.clone(), view.perf_runner)
        };
        if let Some(sink) = perf {
            let already = sink.lock().map(|guard| guard.is_some()).unwrap_or(true);
            if !already && let Some(runner) = perf_runner {
                let capture = self.view.update(cx, |view, _cx| runner(view, window));
                if let Ok(mut guard) = sink.lock() {
                    *guard = Some(capture);
                }
            }
        }

        // `--dump-select` (group 5): same reason, same place -- the probe drives
        // two `EntityInputHandler` calls, and it must see the geometry this frame
        // painted. It restores the buffer before it returns.
        let select = self.view.read(cx).select_probe.clone();
        if let Some(sink) = select {
            let already = sink.lock().map(|guard| guard.is_some()).unwrap_or(true);
            if !already {
                let capture = self.view.update(cx, |view, cx| {
                    crate::selection::select_probe(view, window, cx)
                });
                if let Ok(mut guard) = sink.lock() {
                    *guard = Some(capture);
                }
            }
        }
    }
}

struct PrepaintState {
    /// Selection highlight, element-local. Painted before everything else.
    highlight: Vec<QuadCmd>,
    quads: Vec<QuadCmd>,
    objects: Vec<ObjectCmd>,
    texts: Vec<TextCmd>,
    /// Caret bar, element-local.
    caret: Option<QuadCmd>,
}

/// Which fragments of a block are painted —exposed for the dump and for tests.
pub fn painted_spans(layout: &DocLayout) -> Vec<&DrawSpan> {
    layout.blocks.iter().flat_map(|b| b.spans.iter()).collect()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    // No glob import of gpui here: it exports a `test` module, which would shadow
    // the `#[test]` attribute (the expansion recurses into that module).
    use std::sync::Arc;

    use gpui_kit::gpui::{point, px};

    use super::{caret_scroll_target, insertion_range, normalize_newlines};
    use typors_core::doc::{BlockKind, Document};
    use typors_render::{ApproxMetrics, DocLayout, EditorTheme, LayoutMode};

    /// The reported symptom was "CR is not a line break". Nothing downstream
    /// treats `\r` as one, so the buffer must never receive one.
    #[test]
    fn crlf_and_cr_enter_the_buffer_as_lf() {
        assert_eq!(normalize_newlines("A\r\nB").as_ref(), "A\nB");
        assert_eq!(normalize_newlines("A\rB").as_ref(), "A\nB");
        assert_eq!(normalize_newlines("\r\r\r").as_ref(), "\n\n\n");
        assert_eq!(normalize_newlines("A\nB\r\nC").as_ref(), "A\nB\nC");
        assert_eq!(normalize_newlines("A\nB").as_ref(), "A\nB");
        // The common case must stay allocation-free.
        assert!(matches!(
            normalize_newlines("plain\n"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    /// Ten Enters make ten newlines only if an insertion at the caret is really an
    /// insertion: an insertion range that is not empty would make each Enter
    /// replace the previous one.
    #[test]
    fn an_insertion_at_the_caret_is_never_a_replacement() {
        assert_eq!(insertion_range(Some(5), None), 5..5);
        assert_eq!(insertion_range(Some(0), None), 0..0);
        assert_eq!(insertion_range(None, None), 0..0);
        // A collapsed selection is not a selection.
        assert_eq!(insertion_range(Some(9), Some(9..9)), 9..9);
        // A live selection is replaced.
        assert_eq!(insertion_range(Some(9), Some(4..7)), 4..7);
    }

    /// Consecutive newlines at the end of the buffer must each get their own line
    /// box: three inserted newlines are three blank lines, not one. This is the
    /// layout half of "不能连续换行", and it is the assertion that fails if the
    /// parser or the gap height ever starts collapsing them again.
    #[test]
    fn three_consecutive_newlines_are_three_blank_lines() {
        let mut doc = Document::parse("A\n");
        for _ in 0..3 {
            let at = doc.len_bytes();
            doc.apply_edit(at..at, "\n");
        }
        assert_eq!(doc.text(), "A\n\n\n\n");

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
            .find(|block| block.kind == BlockKind::Gap)
            .expect("the trailing newlines own a blank gap");
        let line = layout.theme.body_size * layout.theme.line_ratio;
        assert!(
            (gap.bounds.size.height.as_f32() - 3. * line).abs() < 0.5,
            "gap height {:.1}, expected {:.1} (3 lines of {line:.1})",
            gap.bounds.size.height.as_f32(),
            3. * line
        );
        assert_eq!(layout.mode, LayoutMode::RENDERED);
    }

    // ------------------------------------------------ caret scroll-into-view

    /// 1000px page, 400px viewport, 20px margin → the band is `scroll+20 ..
    /// scroll+380`, and the scroll range is `0..=600`.
    const PAGE: f32 = 1000.;
    const VIEWPORT: f32 = 400.;
    const MARGIN: f32 = 20.;

    fn target(top: f32, bottom: f32, scroll: f32) -> Option<f32> {
        caret_scroll_target(top, bottom, scroll, PAGE, VIEWPORT, MARGIN)
    }

    /// The half that keeps the fix from being "scroll on every keystroke": a caret
    /// that is already visible must leave the scroll alone.
    #[test]
    fn a_visible_caret_never_moves_the_scroll() {
        // Right in the middle of the band.
        assert_eq!(target(200., 226., 0.), None);
        // Just inside the margins on both sides.
        assert_eq!(target(20., 46., 0.), None);
        assert_eq!(target(354., 380., 0.), None);
        // And the same once the page has been scrolled.
        assert_eq!(target(520., 546., 300.), None);
    }

    /// A caret below the band scrolls down by exactly how far its bottom edge sticks
    /// out, so the whole line box lands on screen and no more.
    #[test]
    fn a_caret_below_the_band_scrolls_down_by_the_minimum() {
        // Bottom edge 100px past the band's bottom edge (380).
        assert_eq!(target(454., 480., 0.), Some(100.));
        // Inside the viewport but inside the bottom margin: still a small scroll.
        assert_eq!(target(370., 396., 0.), Some(16.));
        // One line past the band after the page has already been scrolled.
        assert_eq!(target(700., 726., 300.), Some(346.));
    }

    /// Above the band, symmetrically.
    #[test]
    fn a_caret_above_the_band_scrolls_up_by_the_minimum() {
        // Top edge 50px above the band's top edge (scroll + 20 = 320).
        assert_eq!(target(270., 296., 300.), Some(250.));
    }

    /// The scroll can never leave `0..=page - viewport`, even when the caret is
    /// further out than that.
    #[test]
    fn the_correction_is_clamped_to_the_scrollable_range() {
        // Wants -200, clamps to the top of the document.
        assert_eq!(target(-300., -274., 100.), Some(0.));
        // Wants well past the end, clamps to the last scrollable pixel.
        assert_eq!(target(990., 1016., 0.), Some(600.));
        // Already at the end and still out of band: nowhere to go, so no scroll
        // command at all (otherwise the frame would spin).
        assert_eq!(target(990., 1016., 600.), None);
    }

    /// Degenerate geometry must be answered, not panicked on.
    #[test]
    fn degenerate_geometry_never_panics() {
        assert_eq!(caret_scroll_target(0., 10., 0., PAGE, 0., MARGIN), None);
        assert_eq!(caret_scroll_target(0., 10., 0., PAGE, -5., MARGIN), None);
        assert_eq!(
            caret_scroll_target(0., 10., f32::NAN, PAGE, VIEWPORT, MARGIN),
            None
        );
        // A page shorter than the viewport cannot scroll at all.
        assert_eq!(
            caret_scroll_target(500., 526., 0., 100., VIEWPORT, MARGIN),
            None
        );
        // A margin wider than the viewport is capped at half of it, so the band
        // collapses to a line instead of inverting; the caret then sits just outside
        // it by its own height and the scroll is the height, not a negative band.
        assert_eq!(
            caret_scroll_target(200., 226., 0., PAGE, VIEWPORT, 500.),
            Some(26.)
        );
    }
}
