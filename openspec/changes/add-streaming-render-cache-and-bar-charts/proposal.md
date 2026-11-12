# Proposal

## Why

Opening and scrolling a large Markdown file is slow because every frame walks and materialises **every** block: `DocLayout::layout_mode_marked_cached_media` loops over `doc.blocks()`, and `BlockGeometry::materialize` clones every `DrawSpan` into the new layout even when the viewport shows 20 lines of a 10 MB document. `Document::apply_edit` copies the whole rope into a `String` and reparses the whole document on every keystroke, and `MediaContext::refresh` re-measures every image per layout pass. Nothing survives process exit: decoded images, chart/Mermaid rasterisations and math layouts are rebuilt on every open, and the caret/scroll position the user left is lost. Charts are also incomplete for the most common business use — several `bar` series paint on top of each other, category labels never reach the x axis, negative values render as nothing — and `docs/chart.md` reads as a tutorial rather than a reference an implementer can code against.

## What Changes

- **Streaming (progressive) rendering**: a frame lays out only the blocks the viewport shows (plus a margin) and paints them immediately; the rest of the document is measured by background workers in priority order (nearest to the viewport first) and merged into the layout when ready. Off-screen blocks keep an estimated height so the scroll extent and scrollbar stay stable, and a block that becomes visible is measured on the main thread before it is painted, so no frame ever shows an empty band.
- **Chunked open**: the first paint uses the first chunk of the file; the remaining chunks are parsed and appended in the background. A 10 MB file shows its first page without waiting for the whole parse.
- **Incremental block reparse**: an edit reparses the blocks the edit touches (plus the neighbours whose fence/list state can change), not the whole document. Typing cost stops growing with file size. **BREAKING** for internal callers that assumed `Document::blocks()` is a whole-document stable view: block indices are local to the parsed range until the background parse completes.
- **Per-frame work budget**: layout, shaping and cache writes have a frame budget with a documented degradation order (visible band > margin > background chunks), and the probes report how much of the budget each phase used.
- **On-disk render cache**: rendered resources are cached under `<typors.exe directory>\cache`, with a documented fallback to `%LOCALAPPDATA%\typors\cache` when the executable directory is read-only (Program Files installs). One cache directory per document, named from the document's absolute path, and an index that maps absolute path → cache directory. Cached: decoded image bitmaps, rasterised chart and Mermaid images, math layout output, and per-block layout geometry (heights and folded fragments).
- **Cache correctness and hygiene**: entries are keyed by content, not mtime alone; a changed file, font, theme, DPI or chart source invalidates only what it must; the cache is size-capped with LRU eviction, is safe to delete at any time, and a corrupt entry is ignored and rewritten rather than fatal.
- **Per-file reading position**: on close (and on a periodic checkpoint) typors records the caret byte offset and the vertical scroll offset for the document's absolute path; reopening the same file restores both when the file is unchanged, and clamps into range when it changed.
- **Bar charts**: several `bar` series draw side by side (grouped) instead of overlapping, `x`/`labels` become real category tick labels, values can be printed on top of bars, negative values draw below a zero baseline, and horizontal bars and stacking become selectable options. Other chart types keep their current behaviour.
- **Chart interface reference**: `docs/chart.md` is rewritten as a specification-style interface document — grammar, data model, every global and series option with type/default/range, per-type semantics, axis and legend layout rules, error messages, and a versioned change log — and it must match the implemented parser exactly.

## Capabilities

### New Capabilities

- `editor/streaming-render`: viewport-first layout with background measurement, chunked open, incremental reparse, frame budget and the probes that prove them.
- `editor/render-cache`: on-disk cache location, per-document directory mapping, keying/invalidation, size cap, corruption handling and cache management.
- `editor/document-session`: per-file caret and scroll persistence, restore rules, validation against file changes and the close/checkpoint triggers.
- `editor/charts`: chart block contract — type grammar, series/global options, bar semantics (grouping, category axis, value labels, zero baseline, horizontal, stacked), error reporting, and the reference documentation that must mirror the implementation.

### Modified Capabilities

None. The existing `editor/typora-live-preview` contract (chart and Mermaid fences render on blur) is preserved unchanged; streaming and caching are additions, so no requirement text is modified.

## Impact

- `crates/typors-core`: incremental `parse_blocks` ranges; `Document` gains a parse-progress state (chunk offsets, complete flag) without changing the rope.
- `crates/typors-render`: `DocLayout` becomes a windowed layout (measured/estimated block states, priority queue of unmeasured ranges); `BlockCache` gains a disk tier and stable block keys; `Metrics` gains the `Send + Sync` bound needed for background measurement (or an off-screen fallback, decided in design).
- `crates/typors-editor`: frame orchestration (visible band first, background merge via `cx.notify()`), caret/scroll capture on close, `MediaContext` keys include cache handles.
- `crates/typors-media`: `chart.rs` bar renderer (grouping, ticks, value labels, negatives, horizontal, stacked); chart/math cache entries become serialisable.
- `crates/typors-runtime`: resolve the cache directory, open documents through the chunked path, wire close/checkpoint persistence, call the cache GC on startup and shutdown.
- `crates/typors-probes`: `--dump-perf` gains first-frame/streaming/cache-hit numbers; a new cache probe; chart probe covers bar cases.
- `docs/chart.md`: rewritten as the chart interface reference.
- `README.md`: document the cache location and the streaming behaviour.
- No installer, file-format or settings-schema break: new cache/session files are additive.

## Assumptions

- "柱状图" means completing the existing `bar` type (grouped bars, category labels, value labels, zero baseline, horizontal, stacked), not adding a second chart engine.
- The cache directory is `<exe dir>\cache` when writable, with `%LOCALAPPDATA%\typors\cache` as the fallback for read-only installs.
- All four resource families are cached (image bitmaps, chart/Mermaid rasters, math layouts, block geometry).
- Restoring a reading position restores caret **and** vertical scroll, not the text selection.
- Background measurement is allowed to be approximate only for blocks that have never been on screen; anything painted is always measured exactly.
