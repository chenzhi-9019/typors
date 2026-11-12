# Design

## Context

See `proposal.md` for motivation and the four delta specs for the contract.

Current implementation facts this design must work with:

- `DocLayout::layout_mode_marked_cached_media` (`crates/typors-render/src/render.rs:1097`) always loops over **every** block, calls `BlockGeometry::materialize` per block, and clones every `DrawSpan` into the returned layout. `BlockCache` skips the flow/wrapping pass but not the per-block loop or the clone; `DocLayout` is therefore always whole-document.
- Culling already exists but only at paint time: `shape_frame` (`crates/typors-editor/src/view.rs:2759`) keeps visible fragments, drops the rest, and `DocLayout.culled_fragments` records it. Layout still paid for everything.
- `Document::apply_edit` (`crates/typors-core/src/doc.rs:195`) runs `self.rope.to_string()` and `parse_blocks` over the whole document on every keystroke.
- `MediaContext` (`crates/typors-editor/src/media.rs:52`) caches diagrams/charts/rasters in memory only, with hard clears at 32/64 entries; nothing survives process exit.
- `MathEngine` (`crates/typors-media/src/math.rs:93`) already returns SVG strings plus metrics, so math output is trivially serialisable.
- `Settings` (`crates/typors-settings/src/lib.rs`) is a serde model at `%APPDATA%\typors\settings.json` with an atomic temp+rename save. `Files.restore_last_session` exists but restores no position.
- `RenderImage` exposes `as_bytes(frame)` and `RenderImage::new` (`gpui-pre` `assets.rs`), and the `image` crate 0.25 is already in the tree, so decoded bitmaps can be round-tripped.
- The product ships as `typors.exe` (loader) + `typors_runtime.dll`; the cache root must be resolved against the EXE, not the DLL.

## Goals / Non-Goals

**Goals:**

- First painted frame depends on the viewport, not on document size.
- Whole-document cost becomes background work that never blocks input.
- A warm reopen of an unchanged document skips decoding, rasterising, math layout and block flow.
- Reading position survives restart and is clamped safely on a changed file.
- Bar charts are usable for real category comparisons, and `docs/chart.md` becomes the interface contract.

**Non-Goals:**

- Editing performance inside a single enormous *block* (a 5 MB single paragraph still flows as one block).
- Background threads for parsing inside `apply_edit`; the edit path stays synchronous and local.
- Caching the rendered window bitmap or the GPUI scene.
- Multi-document tabs or session restore of window geometry (only caret + scroll per file).
- Touching `editor/typora-live-preview`: chart/Mermaid fences still render on blur only.

## Decisions

### 1. Windowed layout: measured slots + estimates, not a whole-document layout

`DocLayout` becomes a sparse, windowed structure:

```rust
pub struct DocLayout {
    blocks: Vec<BlockBox>,   // one per block, in document order (unchanged shape)
    measured_blocks: usize,  // blocks with exact, paintable geometry
    estimated_blocks: usize, // placeholders: estimated height, no paint commands
    ordinals: Vec<Option<usize>>, // list ordinals, so a later measurement cannot renumber
    window: Option<Viewport>,
    // ... origin, size, mode, theme, metrics as today
}

pub struct BlockBox {
    // ... unchanged fields
    pub measured: bool,      // false => estimated height, empty spans/decors/objects
}
```

**Implemented representation.** A separate `BlockSlot` enum was rejected during
implementation: roughly a hundred call sites index `layout.blocks[i]` expecting a
`BlockBox` (hit testing, caret, selection, every probe). The placeholder *is* the
estimated slot; `measured: false` carries the distinction, and a placeholder has
empty `spans`/`decors`/`objects` so it can never emit paint commands.

The layout window is `viewport ∪ margin` (the existing `CULL_MARGIN`), measured synchronously before paint. Everything else is a slot with an estimated height: the deserialised disk geometry when the cache has one, otherwise the running per-kind average (`Heading{level}`, `Paragraph`, `Code`, `Table`, …). `extent` is maintained as a running sum, and a Fenwick-style prefix array gives `top(block)` in O(log n) so caret, hit testing and scroll anchoring stay cheap.

**Alternative rejected:** keep a whole-document `DocLayout` and only move *shaping* to a worker. The block loop plus `materialize` clone is O(document) per frame, so first paint and every scroll frame would still be linear in file size.

**Alternative rejected:** virtualise by fixed-height rows. Markdown blocks have wildly different heights (images, tables, headings); estimates from per-kind averages plus disk geometry keep the scrollbar honest without a fixed-height model.

### 2. Background measurement: a budgeted cooperative pass with generation stamps

A `LayoutScheduler` owns:

- a priority order of unmeasured blocks, nearest to the visible band first (`DocLayout::unmeasured_near`);
- a chunked executor that calls `block_geometry` for at most 24 blocks per frame and writes the results into `BlockCache`;
- an `Arc<AtomicU64> generation` bumped by anything that invalidates geometry (revision, width, font tag, theme tag, media generation). A result whose generation is stale is discarded;
- a `std::sync::mpsc` back to the view, drained at the start of the next `prepaint`; a drain that installed at least one chunk calls `cx.notify()` through a main-thread task.

**Metrics thread-safety (resolved).** `Metrics` gained `Send + Sync` and `WindowMetrics` passes a compile-time assertion. That is *not* enough for worker threads, because layout also needs `MediaMeasures`, and the shipped implementation (`MediaContext`) owns GPUI asset state, a Mermaid engine and chart caches that are window-bound. The scheduler therefore uses the designed **cooperative main-thread executor**: `request_layout` spends a bounded slice (24 blocks / 4 ms) measuring the nearest unmeasured blocks into the block cache and calls `cx.notify()` while work remains. Frames stay responsive because visible-band work always runs first and background work strictly after it; no thread-safety assumption is made about GPUI state.

**Alternative rejected:** one thread per document. Chunk ordering by distance is what keeps "scroll into unmeasured content" responsive; independent threads would measure far-away blocks first.

**Alternative rejected:** `AsyncApp` background task per block. Fine-grained tasks drown the executor and each needs a separate round-trip to install geometry; chunks amortise the hand-off.

### 3. Scroll anchoring keeps the viewer still

The first visible block is the anchor. When a background result changes an estimate for a block **above** the anchor, the scroll offset is adjusted by the same delta (`scroll -= Δ`), and the extent is recomputed. The offset is clamped to `0..=extent - viewport`. A block that becomes visible is always measured before paint, so a measured block never replaces a painted estimate. This is what makes "scrollbar settles without oscillating" testable: the anchor's top stays constant across merge frames.

### 4. Streaming parse: one parse, incremental publication, local reparse for edits

**Open (implemented as prefix + whole-file replacement).** `Document::parse_progressive(text, chunk)` parses a safe prefix (a line boundary outside a fence) synchronously, so the first frame has blocks to paint; a worker thread then parses the whole file with the same parser and hands the completed table back over a channel. `finish_progressive` installs it only if the document is still parsing (an edit promotes the document to a whole-file parse and discards the background result). The completed model is exactly `Document::parse`, which is what the spec's "identical to a whole-file parse" requires, without handing the rope across threads.

**Edit.** `apply_edit` computes the affected block(s) and expands to the enclosing *parse unit*: the maximal run of non-gap blocks between two blank-line gaps, plus the trailing gap. Units are derived from the gap blocks already in the table (no stored `Block::unit` field), so the public `Block` shape is unchanged. The unit is sliced out of the rope and reparsed together with one following block as **lookahead** (a paragraph's last line can only be understood against the line after it), then spliced back. A splice is accepted only when the unit is reproduced exactly (contiguous, no block crossing the boundary, the next block unchanged), the fence-line count is unchanged, and the edit does not delete the separator newline; otherwise the whole-document parse runs. `last_edit_blocks` reports the real cost, and a seeded 200-edit test plus a large-document test compare every splice against a fresh whole-document parse.

**Alternative rejected:** reparse the prefix up to the caret. It still copies O(caret) and re-flows everything above; local units are bounded by paragraph size.

**Alternative rejected:** incremental Markdown parsers (e.g. tree-sitter). New dependency, different parser semantics, and the existing `parse_blocks` contract (pulldown-cmark 0.13.5 quirks documented in `doc.rs` tests) would drift.

### 5. Frame budget and degradation order

Per frame: (a) measure the visible band + margin, (b) shape visible fragments, (c) run at most one background chunk of 24 blocks with a 4 ms budget, (d) request another frame while unmeasured blocks remain. A frame never starts (c) before (b) is done; exhausting (c) simply defers to the next frame. `--dump-perf` reports the first-visible-frame time, measured vs estimated counts, background rounds and blocks, bytes unparsed, and cache hits.

### 6. Cache layout, naming and mapping

```text
<cache root>/                         # <exe dir>cache, else %LOCALAPPDATA%	yporscache
  index.json                          # { version, root, documents: { "<canonical path>": { dir, last_used, bytes } } }
  docs/<dir>/meta.json                # { path, size, hash, scale, font_tag, created, last_used }
  docs/<dir>/img/<key>.bin            # decoded BGRA bitmap
  docs/<dir>/raster/<key>.bin         # chart / Mermaid raster
  docs/<dir>/math/<key>.bin           # math SVG + metrics
  docs/<dir>/layout/<key>.bin         # BlockGeometry
```

`<dir>` is `<sanitised file stem>-<16 hex chars of FNV-1a(canonical path)>`, e.g. `notes-3f9a1c07b2d84e51`. The canonical absolute path is stored both in `index.json` and in that directory's `meta.json`, so the mapping is readable and repairable: on startup, a directory whose `meta.json` path is missing from `index.json` is re-adopted, and an index entry whose directory is gone is dropped. FNV-1a is used instead of `DefaultHasher` because `DefaultHasher` is explicitly not stable across Rust releases, and stable names must survive upgrades. Case differences (`C:\A.md` vs `c:\a.md`) are distinct keys because the canonical path string is distinct; the second one simply gets its own directory.

Resource keys are `FNV-1a(resource hash ‖ width ‖ height ‖ scale bits ‖ font tag ‖ theme tag)`. Geometry keys additionally include the block's kind tag, render hash, source hash, ordinal and media generation, matching today's `BlockKey`.

### 7. Cache validation and invalidation

A document entry is valid when `meta.json` records the same absolute path, byte size and content hash as the file just read (computed once during open, on the streaming read thread). Per-resource validation is by key: image entries key on the *source bytes hash* (local file content, URL + response bytes, or data URI), not on mtime, so touching a file does not invalidate anything; chart/Mermaid entries key on the fence source plus raster size and scale; math entries key on the TeX plus font size; geometry entries key on content plus font/theme/width. A change to any key misses only that entry.

`meta.json` is rewritten with the new size/hash after a successful open so the next open can validate cheaply. A file whose content hash differs invalidates the whole document directory (a full clear is cheaper and safer than per-entry diffing on a large rewrite) and the directory is repopulated lazily.

### 8. Cache writes, concurrency and GC

Every entry is written to `<key>.tmp` then renamed over the final name, so readers never see a partial file. Each entry starts with a magic + format version + payload length + FNV checksum; a mismatch is treated as a miss and the entry is rewritten. Reads are tolerant: `EACCES`, `EROFS`, missing files and I/O errors all become misses, and the cache degrades to the existing in-memory maps for the session.

A startup GC thread reads `index.json`, sums directory sizes, and while over the cap evicts the least-recently-used document directories, skipping any directory currently open (tracked by a process-wide `open_documents` set). Default cap: 512 MB, configurable with `files.cache_max_mb` (new settings field, serde default 512, range 64–8192) and switchable with `files.cache_enabled` (default true). GC never runs on the first-paint path.

### 9. What is cached per family, and the size guard

- **Images**: decoded BGRA bitmap (width, height, bytes) for sources up to 4 megapixels; larger images are decoded on demand as today, because a 12 MP photo would cost 48 MB of cache for one picture. Remote images: only the decoded bitmap is cached, not the response bytes, so a cache cannot resurrect a deleted remote resource or leak unrelated content.
- **Charts / Mermaid**: the rasterised BGRA at the exact device size, which is what the current in-memory `diagram_image`/`chart_image` maps hold.
- **Math**: the SVG string and `MathMetrics`, avoiding RaTeX layout on reopen.
- **Geometry**: `BlockGeometry` (spans, objects, decors, heights) serialised as text + ranges + f32s; on a hit, a block's flow is skipped entirely, including text shaping. This is the entry that makes a warm reopen of a long document fast with the document model still being parsed.

### 10. Reading position store

`%APPDATA%\typors\sessions.json`, deliberately separate from the cache (clearing the cache must not lose reading positions, and a read-only exe directory must not disable them):

```json
{ "version": 1,
  "documents": {
    "C:\\work\\notes.md": { "caret": 1234, "scroll": 880.5, "size": 45678, "hash": "9f2c...", "updated": 1760000000 }
  } }
```

Writes reuse the settings pattern (temp file + rename) and are debounced: recorded on window close and app exit, at most every 30 s while movement continues, and on focus loss. Restore happens after the document content is available and before the first layout: `caret = doc.snap_floor(min(caret, len))`, `scroll = clamp(scroll, 0, extent - viewport)`, then a normal `scroll_caret_into_view` pass so the caret is visible. If the recorded size/hash differ, the offsets are only clamped and the record is refreshed. Entries for missing files are dropped after 30 days, and the store is capped at 500 entries by LRU.

### 11. Bar renderer: a dedicated, metric-free implementation

`typors-media` has no text metrics, so label layout uses the same character-class approximation as `ApproxMetrics` (CJK = 1 em, ASCII = 0.55 em). The new `bar()`:

- **Groups**: `slot = plot_width / categories`, group width `= 0.72 * slot`, bar width `= clamp(group_width / series_count - 2, 2, 36)`, series offset symmetric around the category centre, ordered by series name (the existing `BTreeMap` order).
- **Value axis**: `min = min(0, values)`, `max = max(0, values)`; when both are 0 the range becomes `0..1`; five evenly spaced ticks; the zero line is drawn heavier when the range crosses zero.
- **Categories**: `labels` global > first series with `x`/`labels` (series order) > 1-based indices. A label wider than its slot is truncated with `…`; labels are centred under the slot.
- **Negative values**: a bar is a rect from `y(0)` to `y(value)`, so negatives grow downward.
- **`horizontal: true`**: swap axes; categories down the left with an 88 px label gutter, value ticks along the bottom, bars extend right (positive) or left (negative) from x(0).
- **`stacked: true`**: per category, positive segments accumulate upward from zero, negative segments downward; the segment rect spans the previous cumulative edge to the new one.
- **`values: true`** (global or per series): value text at the bar end, middle-anchored; skipped when the segment is under 12 px in the value direction or the text would leave the plot area.
- Every produced number is rounded to 1 decimal for display, with integers printed without a decimal point.

**Alternative rejected:** keep the shared `cartesian()` and add mode flags. Grouping, stacking, a categorical axis and a horizontal orientation invert enough of the geometry that a shared function would be a pile of `if kind == Bar` branches; `line`/`scatter` stay on the existing path.

### 12. Chart option parsing: reserved globals, explicit errors, compatibility

`parse()` gains a reserved global set (`title`, `width`, `height`, `labels`, `stacked`, `horizontal`, `values`) matched before the `series.option` split. Unknown *options* (a key containing `.` whose option is not in the documented table) become errors naming the option and line, instead of being silently ignored; unknown *bare* keys remain series names, and that gotcha is documented. Existing documents that only use documented forms keep rendering; the behaviour change is the grouped/labelled `bar` output, recorded in the doc change log as a minor version bump.

### 13. Documentation as a checkable contract

`docs/chart.md` is restructured as a reference: 概要, 语法 (grammar block), 数据类型, 全局选项表, 系列选项表, 类型 (pie/line/bar/scatter), 柱状图 (grouping/labels/values/negative/horizontal/stacked), 坐标轴与刻度, 图例, 错误, 兼容性, 变更记录. Two checks keep it honest:

1. `typors_media::chart::OPTIONS` (a `&'static [(&str, Scope)]` inventory) is compared with the option names found in the documentation tables — a missing or extra name fails the test.
2. A test extracts every fenced `chart` block from `docs/chart.md` and renders it, failing on any error. This runs in `cargo test -p typors-media` with the doc path resolved from `CARGO_MANIFEST_DIR`.

### 14. Observability and probes

- `--dump-perf`: adds `first_visible_ms`, `measured`/`estimated` slots, `pending_chunks`, `unparsed_bytes`, and cache hit counts; keeps the existing layout/shaping split.
- New `--dump-cache`: prints the resolved root, per-document directory names, entry counts and bytes, and runs a cold→warm round-trip that asserts identical geometry fingerprints.
- New `--dump-session`: records a position, reloads the store, reopens the document, asserts caret/scroll.
- `--dump-chart`: bar matrix (grouped, stacked, negative, horizontal, value labels, truncation) plus the doc examples.
- Existing probes: each accepts a streaming-disabled switch (`TYPORS_STREAMING=off`) and must produce the same geometry as the streaming path once fully measured.

## Risks / Trade-offs

- [Background work needs window-bound media state] → `Metrics` is `Send + Sync`, but media measurement is not; the cooperative main-thread executor is used, visible-band work first and background work strictly after.
- [Estimate drift makes the scrollbar jump] → anchor compensation on merges, visible blocks measured before paint, and a probe that measures anchor movement across a full background pass.
- [Local reparse diverges from whole-document parse] → boundary validation plus fallback to the full parse; randomized edit tests compare both paths.
- [Streaming parse publishes blocks the caret cannot address yet] → publication is append-only and the caret is clamped to published content during open; the caret is placed before the first paint anyway.
- [Disk cache corruption or partial writes] → magic/version/checksum + atomic rename; every failure path is a miss.
- [Cache grows unbounded] → 512 MB default cap, startup LRU GC, open documents exempt, status/clear commands.
- [Program Files is read-only] → documented LOCALAPPDATA fallback; the cache never blocks opening.
- [Background work competes with input] → 24 blocks / 4 ms per frame, nearest-first, and skipped entirely when the visible band used the frame.
- [Bar behaviour change surprises existing documents] → documented change log entry and a version bump; `line`/`scatter`/`pie` output is unchanged and covered by regression tests.
- [Geometry disk cache grows per block key] → keyed per block content and capped like every other family; a cold miss simply re-flows the block.

## Migration Plan

1. Ship layout/scheduler behind `TYPORS_STREAMING` (default on, `off` for probes and one-release rollback).
2. Ship disk cache with `files.cache_enabled = true`; a missing or unwritable root is a silent no-op, so rollback is "delete the cache directory".
3. Session store is additive; deleting `sessions.json` is the rollback.
4. Bar changes land with the doc rewrite in one commit; the change log records the version.
5. README documents the cache location, the clear/status commands and the position store.
