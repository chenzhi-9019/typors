# Tasks

## 1. Baseline and failing evidence

- [x] 1.1 Add an empty-document regression test in `typors-core` asserting `Document::parse("")` has one block at `0..0`, `block_at_src(0)` is `Some(0)`, and `src_to_render(0)`/`render_to_src` round-trip; run `cargo test -p typors-core empty_document` and record the current failure.
- [x] 1.2 Add caret geometry tests in `typors-render` for offset 0, line start after `\n`, trailing newline, three consecutive blank lines, object right edge, heading/code line height, and 2.0 logical-px width; run `cargo test -p typors-render caret_` and record which cases fail.
- [x] 1.3 Add a `--dump-blank` probe skeleton that opens an empty document, clicks page center, types `你`, presses Enter and commits a fake IME composition; assert current behavior fails and capture its output in the change notes.
- [x] 1.4 Document the reproduced symptoms (blank doc no caret, click no caret, trailing-line caret on previous line, ghost after scroll) in `design.md` Open Questions or the change notes, then close the baseline task.

## 2. Empty document model

- [x] 2.1 Normalize empty/all-whitespace parses to one zero-length `Paragraph` block in `typors-core`, preserving the block tiling invariant; run `cargo test -p typors-core`.
- [x] 2.2 Make `Document::apply_edit` keep that single block after the first insertion and after deleting back to empty; verify `cargo test -p typors-core` and a new edit-empty-edit test.
- [x] 2.3 Verify `DocLayout` lays out the zero-length block as one caret line at the page top with no glyph paint; add a layout test asserting one block and positive line height.

## 3. Caret line model

- [x] 3.1 Add `line_bounds` and `line_index_for_byte` to `DocLayout` using the existing shaped fragments and `line_tops`; unit-test that every byte of a multi-line sample maps to a line whose start/end agree with `src_to_render`.
- [x] 3.2 Replace `caret_x`'s span fallback with line-based lookup: use the owning line's fragment at the byte, or `content_left` for an empty line, and a line-start byte uses `content_left`; run the caret geometry tests from 1.2.
- [x] 3.3 Make `caret_bounds` use the owning line's height for headings, code, quotes and blank runs; add assertions for heading/code/blank line heights.
- [ ] 3.4 Add a caret paint test that renders two consecutive frames after a scroll and asserts exactly one caret at the new bounds and none at the old bounds; verify with `cargo test -p typors-render caret_paint`.

## 4. Blank document editing and reset

- [x] 4.1 Initialise `MarkdownView` with `caret: Some(0)` for an empty document and make `on_mouse_down` fall back to `(block 0, 0)` when `hit` is `None`; verify with `--dump-blank` and a view-level test.
- [x] 4.2 Add `MarkdownView::replace_document(Document)` that resets caret/anchor/selection/marked/scroll/desired_x/history and invalidates layout; test that New/Open paths leave no stale caret or selection.
- [ ] 4.3 Verify printable input, Enter, paste and IME commit/cancel on a blank document with `--dump-blank`, including candidate anchoring at offset 0; run the probe to exit 0.
- [ ] 4.4 Verify focus round-trip: unfocused window hides the caret, refocus shows it at the same offset, menu open/close does not duplicate it; add the view test and run it.

## 5. Menu model and commands

- [x] 5.1 Create `MenuState`/`MenuCommand`/`MenuSpec` in `crates/typors-runtime/src/menu.rs` with the seven menus and the item lists from design.md; unit-test labels, order and shortcut text.
- [x] 5.2 Implement `MenuState::refresh_availability` for undo/redo, copy/cut, save/save-as and no-caret states; unit-test enabled/disabled transitions after typing, selecting and saving.
- [x] 5.3 Implement keyboard highlight movement that skips disabled items, Enter/Space dispatch, Left/Right menu switching and Esc close; unit-test each transition.
- [x] 5.4 Add `MarkdownView::apply_editor_command(EditorCommand)` for paragraph/format commands, reusing the existing insertion/history paths; test bold/list/heading commands and one undo round-trip.

## 6. Editor shell and menu rendering

- [x] 6.1 Add `EditorShell` in `crates/typors-runtime/src/shell.rs` owning `Entity<MarkdownView>`, `MenuState`, `FileState` and focus handles; make `open_window` return the shell and verify the existing window opens with a 28 px menu row.
- [x] 6.2 Render the menu bar with theme colors, hover/highlight states and a fixed row above a `flex_1` editor; verify the page does not scroll under the bar and the caret viewport remains correct.
- [x] 6.3 Render dropdowns as an overlay layer with a backdrop, paint above the page and close on outside click; verify mouse open/close and that no document edit occurs.
- [x] 6.4 Wire Alt+F/E/P/O/V/T/H mnemonics and focus return after Esc/command; verify typing `abc` immediately after closing inserts at the editor caret.
- [x] 6.5 Add a menu smoke probe (or unit-level shell harness) that opens each menu, checks item counts and closes it; run it to exit 0.

## 7. File commands

- [x] 7.1 Add `crates/typors-runtime/src/file_dialog.rs` wrapping native Open/Save dialogs with Markdown filters and UTF-16 paths; unit-test filter construction and cancel mapping.
- [x] 7.2 Implement `FileState` dirty/saved-revision tracking and a native Save/Discard/Cancel guard for New/Open/Exit; test all three answers with an injectable prompt function.
- [x] 7.3 Implement New, Open, Save and Save As in `EditorShell`, including title updates, read/write failure handling and no buffer mutation on failure; verify with a temp-file test and manual menu run.
- [x] 7.4 Verify Save with no path opens Save As, Save As cancel is a no-op, and saving an existing path writes the exact buffer text; add the temp-file round-trip test.

## 8. Integration, probes and docs

- [x] 8.1 Extend `--dump-cursor` with line-start/end/trailing-newline/blank-run cases and keep `--dump-blank` green; run all `--dump-*` modes and record exit codes.
- [x] 8.2 Run `cargo test --workspace`; confirm A1–A5, E/I/N/S/P probes are unchanged and the new menu/blank/caret tests pass.
- [x] 8.3 Update `README.md` and `docs/` with the menu categories, keyboard mnemonics, New/Open/Save behavior and the blank-document editing contract; verify the documented commands run as written.
- [x] 8.4 Verify release build (`cargo build --release --workspace`) and a real-window manual checklist: launch without a file, type, Enter, IME, scroll, menu open/command/Esc, New/Open/Save; attach screenshots to the change notes.

## Workflow follow-up

- Review the artifacts before implementation; this proposal intentionally stops at planning.
- After implementation and review, archive the change and re-run `openspec validate --all --strict`.
