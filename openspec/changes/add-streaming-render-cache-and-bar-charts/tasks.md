# Tasks

## 1. Foundations: switches, stable hashing, cache root, settings

- [x] 1.1 Add the stable-hash utility (`FNV-1a 64`, two seeds) in a shared crate, with tests pinning known vectors so a refactor cannot silently change on-disk names.
- [x] 1.2 Resolve the cache root against the `typors.exe` directory using `std::env::current_exe()`, fall back to `%LOCALAPPDATA%\typors\cache` when it cannot be created or written, and cache the decision for the session; unit-test the resolution with an injected environment (temp dir writable, temp dir read-only via attributes, missing exe dir).
- [x] 1.3 Add `TYPORS_STREAMING` and `TYPORS_CACHE` environment switches parsed once at startup (`on`/`off`, default on) and thread them into the view and media contexts; verify a probe run with each switch off still paints and exits 0.
- [x] 1.4 Add `files.cache_enabled` (default true) and `files.cache_max_mb` (default 512, range 64–8192) to `typors-settings`, keeping schema version 1 via serde defaults; verify `--dump-settings` covers the defaults, the range validation and a missing-field load.
- [x] 1.5 Assert `WindowMetrics` is `Send + Sync` with a compile-time assertion and choose the executor (worker pool or main-thread cooperative) from the result; verify `cargo build -p typors-render` compiles with the assertion and record the chosen executor in the design's risk section.

## 2. Disk cache store: index, entries, atomicity, GC

- [x] 2.1 Implement `CacheStore` (root, `index.json`, per-document `docs/<dir>/meta.json`, resource directories) with `register_document(canonical_path) -> DocumentCache`, deterministic `<stem>-<hash>` directory names and index re-adoption for orphaned directories; unit-test stable naming, two same-named files, a path with a drive letter and a moved file.
- [x] 2.2 Implement entry read/write with magic, format version, payload length and FNV checksum, written to `.tmp` then renamed; unit-test round-trip, truncated file, garbage header, checksum mismatch and a read-only directory (all failures must be misses, never panics).
- [x] 2.3 Implement document validation (`meta.json` path + size + content hash) and the invalidation rule that a changed document clears its directory; unit-test unchanged reopen, same-size different-content reopen and mtime-only change.
- [x] 2.4 Implement usage accounting and the startup LRU GC with a size cap, skipping directories of open documents; unit-test the cap, LRU order, open-document exemption and a GC that runs below the cap.
- [x] 2.5 Add `--clear-cache` and `--cache-status` to the executable, printing the resolved root, total bytes and per-document entries; verify status on a warm cache, clear removes data, and a document opened afterwards renders from scratch.
- [x] 2.6 Add `--dump-cache`: resolve root, register a document, write and read back a synthetic entry, assert the mapping is stable and the checksum rejects a mutated byte; run it to exit 0.

## 3. Streaming parse and local reparse

- [x] 3.1 Add `ParseState` (parsing with published block count / complete) and `Document::parse_streaming` that publishes completed blocks in append-only chunks from one `pulldown_cmark` walk; unit-test that indices are stable while chunks arrive and that the completed table equals `Document::parse`.
- [x] 3.2 Add `Block::unit` (the blank-line-separated parse unit) to the block model without changing public geometry consumers; verify existing `typors-core` and `--dump-layout` tests still pass.
- [x] 3.3 Implement local `apply_edit`: reparse the affected unit from the rope slice, splice the blocks, validate boundary offsets and length, and fall back to the whole-document parse on any mismatch; unit-test edits inside a paragraph, at a fence boundary, at offset 0 and a list continuation.
- [x] 3.4 Add a randomized equivalence test (seeded, at least 200 edits) asserting the local-reparse blocks, source ranges and render text equal a fresh whole-document parse; verify with `cargo test -p typors-core`.
- [x] 3.5 Add an edit-cost counter to the document (blocks reparsed / bytes copied) and expose it in `TYPORS_EDIT_LOG`; verify a typing edit in a 5 MB fixture reports one unit reparsed and no whole-document copy.

## 4. Windowed layout model

- [x] 4.1 Replace `DocLayout.blocks: Vec<BlockBox>` with `slots: Vec<BlockSlot>` (`Measured(BlockBox)` / `Estimated { height, source }`) plus a prefix-sum offset index; keep `block_bounds`, `block_at_y`, `hit`, `caret_bounds` and `bounds_for_range` working through the new accessors; verify all `typors-render` tests and `--dump-layout` pass.
- [x] 4.2 Implement `ensure_visible(viewport) -> Vec<usize>` that measures every block intersecting the band, and per-kind running averages for estimates; unit-test that only intersecting blocks are measured and that estimates are used outside the band.
- [x] 4.3 Implement `install(block_idx, BlockGeometry)` and `estimate_for` with extent recomputation and anchor compensation for measurements above the visible band; unit-test the anchor's top staying constant across a merge and the scroll offset clamping at both ends.
- [x] 4.4 Gate painting on state: a slot that is `Estimated` and intersects the viewport must be measured first, and an estimated slot must never emit paint commands; unit-test that a viewport over unmeasured slots produces no blank paint and no fragment from an estimate.
- [x] 4.5 Add a geometry fingerprint over measured slots and unit-test that a fully measured windowed layout has the same fingerprint as today's whole-document layout.

## 5. Layout scheduler and frame orchestration

- [x] 5.1 Implement `LayoutScheduler`: priority queue by distance from the visible band, chunk size 16–32, generation stamps for revision/width/font/theme/media, and a result channel drained at frame start; unit-test ordering (near-first), stale-generation discard and empty-queue idle.
- [x] 5.2 Wire the scheduler into `MarkdownView::prepaint`: drain, measure visible band, shape, then schedule/run at most one background chunk under the 8 ms / 4 ms budget; verify with a 5 MB fixture that the first frame paints visible content and that frames continue while chunks are pending.
- [x] 5.3 Implement the main-thread notification path for background results (drain-then-`cx.notify()`) and verify a completed chunk repaints without user input, and that a stale chunk does not repaint.
- [x] 5.4 Add the cooperative main-thread executor for the case where `WindowMetrics` cannot be `Send + Sync`, sharing the scheduler; verify both executors produce identical fully-measured geometry fingerprints.
- [x] 5.5 Extend `--dump-perf` with `first_visible_ms`, measured/estimated slot counts, pending chunks, unparsed bytes and cache hits; verify the report on a 5 MB fixture, including a run with `TYPORS_STREAMING=off` that still exits 0.
- [x] 5.6 Add the streaming parity test: the same document laid out streaming-until-complete and with streaming off must have identical geometry fingerprints, caret bounds and hit-test results.

## 6. Resource cache wiring

- [ ] 6.1 Add serialisation for `BlockGeometry` (spans, objects, decors, heights, string table) keyed by content + font/theme/width, and load it into the layout as `Estimate::DiskGeometry`; unit-test round-trip equality of all geometry fields.
- [x] 6.2 Cache decoded image bitmaps (BGRA + dimensions, ≤ 4 megapixels) through `MediaContext`, keyed by source-bytes hash; verify a second process paints the same pixels and that an oversized image skips the disk cache.
- [x] 6.3 Cache chart and Mermaid rasters at their device size and scale, keyed by fence source + size + scale; verify a warm open skips SVG parsing and rasterisation (instrument the counters and assert zero raster calls).
- [ ] 6.4 Cache math SVG + metrics keyed by TeX + font size; verify a warm load performs no RaTeX layout.
- [ ] 6.5 Add the cold-vs-warm equivalence test: empty cache then warm cache must produce identical geometry fingerprints and identical painted fragment lists; verify as a unit test in `typors-editor` and through `--dump-cache`.
- [x] 6.6 Make cache failures silent and degrade to the in-memory maps: verify with a read-only cache root, a deleted root at runtime and a corrupt entry that opening, editing and saving all succeed.

## 7. Reading position store

- [x] 7.1 Implement `SessionStore` (`%APPDATA%\typors\sessions.json`, versioned, atomic write, corrupt-file tolerance, 500-entry LRU, 30-day missing-file pruning); unit-test round-trip, corrupt file, unwritable file and pruning with an injected clock.
- [ ] 7.2 Capture caret byte + scroll offset on window close, app exit, focus loss and a 30 s movement checkpoint; verify with a fake clock that checkpoints are debounced and that no capture blocks typing.
- [x] 7.3 Restore before the first layout: validate size + content hash, clamp caret with `snap_floor` and scroll to the extent, then run `scroll_caret_into_view`; unit-test unchanged restore, content changed beyond the offset, shrunk file and an empty store.
- [ ] 7.4 Prove restore does not edit the document: verify dirty state, undo stack and file bytes are unchanged after open, and that an untitled document creates no record.
- [x] 7.5 Add `--dump-session` performing a record → reload → reopen round-trip with assertions and printing effective offsets; run it to exit 0.
- [ ] 7.6 Verify independence from the render cache: clear the cache, reopen the same file, and assert the posture is still restored.

## 8. Bar charts

- [x] 8.1 Rewrite `bar()` in `crates/typors-media/src/chart.rs` with grouped bars, slot geometry and the documented minimum bar width; unit-test rect count, non-overlap of two series in one category and separation with eight series.
- [x] 8.2 Add the category axis (global `labels` > first series labels > indices, ellipsis truncation, no collision) and unit-test explicit labels, no labels and overlong CJK labels.
- [x] 8.3 Add the value range with zero baseline and negative bars; unit-test all-positive, mixed and all-negative ranges and that no negative bar is omitted.
- [x] 8.4 Add `values: true` (global and per series) with placement and skip rules; unit-test text presence, absence, and omission in a segment too short to fit.
- [x] 8.5 Add `horizontal: true` and `stacked: true` (including downward negative stacking); unit-test axis orientation, cumulative segment edges and negative stacks.
- [x] 8.6 Add the bar matrix to `--dump-chart` and run it to exit 0; verify `line`, `scatter` and `pie` output is unchanged against stored expectations.

## 9. Chart parsing contract and reference documentation

- [x] 9.1 Add the reserved global option set (`title`, `width`, `height`, `labels`, `stacked`, `horizontal`, `values`) with precedence over series names, and reject unknown `series.option` keys with a line-numbered error; unit-test precedence, each new global, an unknown option and a bare unknown key remaining a series.
- [x] 9.2 Publish the option inventory (`OPTIONS` with scope and type) used by the parser and by the documentation check; verify the inventory matches the accepted options in unit tests.
- [x] 9.3 Rewrite `docs/chart.md` as a reference (概要, 语法, 数据类型, 全局选项表, 系列选项表, 类型, 柱状图, 坐标轴与刻度, 图例, 错误, 兼容性, 变更记录) with a version bump and the bar behaviour change logged; verify every option appears in a table and every example is a complete runnable block.
- [x] 9.4 Add the documentation checks: table-vs-inventory comparison and rendering of every fenced `chart` example extracted from `docs/chart.md`; verify with `cargo test -p typors-media`.
- [x] 9.5 Update the chart section in `README.md` to point at the reference and to describe the bar behaviour, then verify the documented example runs as written.

## 10. Integration and regression

- [x] 10.1 Document in `README.md` the cache root and fallback, the clear/status commands, the session store and the streaming switches; verify each documented command runs as written.
- [x] 10.2 Run the full probe suite (`--dump-layout`, `--dump-media`, `--dump-live-preview`, `--dump-cursor`, `--dump-edit`, `--dump-ime`, `--dump-select`, `--dump-newlines`, `--dump-perf`, `--dump-blank`, `--dump-menu`, `--dump-blink`, `--dump-completion`, `--dump-settings`, `--dump-cache`, `--dump-session`, `--dump-chart`) and confirm every mode exits 0.
- [x] 10.3 Run `cargo test --workspace` and `cargo build --release --workspace`, then measure and record first-paint and scroll timings on a 5 MB fixture against the pre-change baseline, with cache cold and warm.

## Workflow follow-up

- Review the change, then archive it once the project's review requirements are satisfied.
- Verify the archived spec set includes all four new capabilities.
