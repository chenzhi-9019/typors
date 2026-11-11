# Design

## Context

See `proposal.md` and the two delta specs. Today `MarkdownView::render` is the window root: a single scrollable page containing `DocElement`. There is no menu row and no document shell, so a menu has nowhere to live without shrinking the editor's viewport. `open_window` loads either a file or `String::new()`; `Document::parse("")` produces no blocks, `DocLayout::hit` therefore has no block to return, and the view starts with `caret: None`. Caret geometry currently derives x/y from a `DrawSpan` and falls back to `block.spans.last()` when the byte falls past the last fragment; that fallback is the shared cause of the stale previous-line caret at trailing newlines, long blank runs and object boundaries.

## Goals / Non-Goals

**Goals:**

- Add a fixed, theme-aware, GPUI-drawn menu bar above the editor, with real command dispatch and focus return.
- Make the no-file startup a first-class editable document and make caret geometry a property of the line box, not of the last span.
- Keep the existing single-buffer / two-projection model and all current probes working.

**Non-Goals:**

- No native Win32 menu (`SetMenu`) and no macOS global menu; the chosen menu is drawn in the GPUI window.
- No new document format, no tabs/multi-window, no project/workspace model.
- No change to Mermaid, LaTeX, image, table or installer pipelines.
- No rich text toolbar; the menu only exposes existing or explicitly listed commands.

## Decisions

### 1. Editor shell owns the menu row

Add a runtime-level `EditorShell` view (`crates/typors-runtime/src/shell.rs`) that owns:

- `Entity<MarkdownView>` — the page and all text editing.
- `MenuState` — open menu index, highlighted item, command availability, recent error.
- `FileState` — current path, saved revision, dirty flag.
- The window focus handles.

`EditorShell::render` is a vertical flex: a 28-logical-pixel `MenuBar` row followed by the editor with `flex_1` and `min_h_0`. `open_window` returns `Entity<EditorShell>` instead of `Entity<MarkdownView>`. The menu row is outside the scroll container, so it never scrolls with the document. `--dump-*` probes keep constructing `MarkdownView` directly, so their geometry is unchanged; only real windows get the 28 px menu.

Alternative: put the menu inside `MarkdownView`. Rejected: the editor should not know about file dialogs, save paths or app commands, and the scroll container would have to be restructured anyway.

Alternative: reuse `gpui-component`'s `AppMenuBar`/`OwnedMenu`. Rejected for this change: it requires `GlobalState` app-menu registration and its popup owns focus in ways that complicate returning to a custom `Element`-based editor; a small shell-local model is easier to test and keeps menu labels/keys exactly as specified.

### 2. Menu is a declarative model plus one popup

`MenuState` holds a static `Vec<MenuSpec>`:

| Menu | Items (implemented in this change) |
| --- | --- |
| 文件 | 新建 `Ctrl+N`, 打开 `Ctrl+O`, 保存 `Ctrl+S`, 另存为 `Ctrl+Shift+S`, 退出 |
| 编辑 | 撤销, 重做, 剪切, 复制, 粘贴, 全选 |
| 段落 | 标题 1–6, 正文, 无序列表, 有序列表, 引用, 代码块 |
| 格式 | 粗体, 斜体, 删除线, 行内代码, 行内公式, 链接, 图片 |
| 视图 | 放大, 缩小, 重置缩放, 切换源码/渲染提示, 全屏 |
| 主题 | 浅色, 深色, 跟随系统 |
| 帮助 | 键盘快捷键, 关于 typors |

Each item is `MenuCommand` (an enum) with label, shortcut text, enabled predicate and checked state. Rendering uses `div()` rows and an absolutely positioned dropdown (`deferred`/`anchored`) so the popup paints above the page and does not participate in the page's layout. The menu model is pure data and can be unit-tested without a window; dispatch is a single `EditorShell::run(command)`.

Paragraph/format commands reuse a new `MarkdownView::apply_editor_command(EditorCommand)` rather than duplicating markdown insertion logic. It operates on the current caret/selection and records normal undo history. File commands stay in the shell.

### 3. Focus and event routing

The menu bar is not a text input. Clicking a top-level label opens the dropdown without moving the text caret, and the shell stores the editor's `FocusHandle` before opening. Closing (Esc, outside click, command) calls `window.focus(&editor_focus)`; this is also what makes "type immediately after closing" work.

`Alt+F/E/P/O/V/T/H` are registered as shell-level key bindings and only intercept when the shell has focus and no IME composition is active. Left/Right/Up/Down/Enter/Esc are handled by the menu state while a dropdown is open. Outside clicks are handled by a full-window backdrop layer under the dropdown, not by stealing focus from the editor.

### 4. Empty document is a real block

`Document::parse("")` (and any all-whitespace document that currently yields zero blocks) will normalize to one zero-length `Paragraph` block at `src 0..0`, with an empty `render` string and no spans. `block_at_src(0)` returns it, so `hit`, `caret_x`, `line_tops` and `blank_line_*` have a block to work with. The block is not painted as a visible glyph; its line box is the page's first line. This keeps the existing "blocks tile the source" invariant (one block, zero length) instead of adding a synthetic layout-only block that has no document counterpart.

`MarkdownView` will initialize `caret: Some(0)` when the loaded document is empty (and on `replace_document` for any document), and `on_mouse_down` will fall back to `(block 0, offset 0)` when `hit` returns `None` and the document has no blocks. The blank page's click target is therefore the whole page above the first line.

### 5. Caret geometry comes from line boxes

`DocLayout` gains an explicit line model per block:

- `line_tops(block_idx)` already exists; extend it with `line_bounds(block_idx, line)` returning `(top, bottom, content_left, content_right)`.
- `line_index_for_byte(block_idx, render_byte)` returns the visual line whose source range contains the byte, treating each `\n` as ending the previous line and starting the next; a trailing `\n` produces an extra empty line.
- `caret_x` becomes: find the line; if the line has fragments, use the fragment at the byte and `x_for_index`; if it is empty, use `content_left` and the line's top; if the byte is exactly at the line start, use `content_left` rather than the previous fragment's right edge.

The old `or_else(|| block.spans.last())` fallback is removed. This is the single fix for the "previous line", trailing-newline and blank-run symptoms; it also gives the correct height for headings/code because the line box is already projection-aware.

Caret bounds remain `size(px(2.), line_height)`. Width stays logical 2 px at every scale; no DPI-specific branch is needed because GPUI's `px` is logical.

### 6. Caret repaint and focus

The caret remains one `QuadCmd` produced in `prepaint` and painted last. The shell calls `cx.notify()` on menu open/close and on focus changes; `MarkdownView` already notifies on caret moves. Because the element is immediate-mode, a caret that is not produced in the current prepaint cannot leave a ghost; the ghost bug is addressed by removing the stale `spans.last()` geometry, not by adding a second paint path. While the window is unfocused, `paint` keeps skipping the caret (existing `focus_handle.is_focused` guard) and the new shell preserves the document offset across focus changes.

### 7. File commands and dialogs

`EditorShell` tracks `path: Option<PathBuf>` and `saved_revision: u64`. Dirty = `view.doc().revision() != saved_revision`.

- 新建: if dirty, native `MessageBoxW` Save/Discard/Cancel; on Save run save first; then `view.replace_document(Document::parse(""))`, clear path, reset saved revision.
- 打开: same dirty guard; native `GetOpenFileNameW` filtered to `*.md;*.markdown;*.mdown;*.txt`; read with the existing `load` helper; on failure keep the buffer and show an error line in the shell.
- 保存: if no path, Save As; write `view.doc().text()` with `std::fs::write`; on success set saved revision and title.
- 另存为: native `GetSaveFileNameW` with a `.md` default extension.
- 退出: dirty guard then `window.remove_window()`/`cx.quit()`.

`crates/typors-runtime/src/file_dialog.rs` wraps `GetOpenFileNameW`/`GetSaveFileNameW` from `windows-sys` (`Win32_UI_Shell`, `Win32_UI_Controls`, `Win32_System_Com` features), using `OPENFILENAMEW`, a 32 KiB buffer and `OFN_EXPLORER | OFN_NOCHANGEDIR`. Dialogs are modal and run on the UI thread; cancel is a normal `None`, not an error. This avoids a new crate and keeps the installer payload unchanged.

### 8. Menu availability and theme

`MenuState::refresh_availability(&MarkdownView, &FileState)` runs on every open and after every command. It disables undo/redo based on `view.can_undo()/can_redo()`, copy/cut on `selection().is_none()`, save on `path.is_none() && !dirty` (Save As remains enabled), and paragraph/format commands when no caret block exists. The bar uses `theme.page_bg`, `theme.text`, `theme.border` and `theme.selection`; no hard-coded colors. The theme menu writes the existing theme selection (or the closest supported state) and notifies the editor; unsupported theme entries are not shown rather than stubbed.

### 9. Testing strategy

- Unit tests for `MenuState`: labels/order, enabled predicates, keyboard highlight skipping disabled items, command dispatch.
- Core tests: empty/whitespace document yields one block, `block_at_src(0)`, edit then parse, mapping round-trip.
- Render tests: caret at offset 0, line start after `\n`, trailing newline, 3 blank lines, object right edge, heading/code line height, high-DPI width remains 2 logical px.
- A new `--dump-blank` probe constructs an empty window, asserts a hit at page center maps to `(0,0)`, types `你`, Enter, and an IME commit, and prints caret bounds per step.
- `--dump-cursor` gains line-boundary cases; existing A1–A5/E/I/N/S/P probes must stay green.
- Menu itself is visually verified in a real window; no automation of native dialogs in CI. The shell exposes `MenuState` tests for dispatch and focus-independent behavior.

## Risks / Trade-offs

- **GPUI popup/focus quirks** → Keep the popup as a shell child with an explicit backdrop and focus handle; add a probe/manual checklist for Esc, outside click and Alt-mnemonic. If `deferred` z-order fights the page, render the dropdown in a sibling overlay layer rather than inside the menu row.
- **Empty-block invariant breaks mapping assumptions** → Add the zero-length block once in core, assert blocks tile and round-trip, and keep layout's blank-line path. Fall back to a layout-only empty line if the core invariant proves too invasive.
- **Line model duplicates existing span layout** → Build it from the same `DrawSpan` list and `line_tops` used by hit testing; add tests that line starts/ends agree with `src_to_render` for every offset in a sample.
- **Native dialogs block the UI thread** → Accept for a desktop editor; use modal dialogs only from menu commands, never from paint/layout.
- **Menu steals Alt combinations used by IME** → Intercept mnemonics only when no composition is active and focus is not in a text input; `Esc` always returns to the editor.
- **Window shell refactor breaks probe geometry** → Probes bypass the shell; keep `MarkdownView` layout independent of the 28 px row, and verify existing `--dump-*` unchanged.

## Migration Plan

No data migration. Ship as a normal feature release; the new menu is additive and the caret/empty-document fixes are behavior corrections. Roll back by reverting the shell/menu and core empty-block changes; saved files are unaffected.

## Baseline evidence

Reproduced before implementation on 2026-11-11:

- `Document::parse("")` produced 0 blocks; `block_at_src(0)` and `DocLayout::hit` returned `None`; the view started with `caret: None`, so the blank page had no visible caret and a click did nothing.
- A paragraph ending in `\n` exposed no line box for the trailing newline; `caret_x` fell back to `block.spans.last()`, so the caret stayed on the previous painted line.
- The caret was a 2 px quad, but its y/height came from the fallback span rather than the owning line, so headings/code/blank runs could use the wrong line height.
- Opening/closing UI changed focus without a shell-level focus restore; the caret disappeared until the editor was clicked again.

## Open Questions

- Exact visual styling of the menu row (font size, hover fill, separator) can follow the existing theme tokens and the reference screenshot; no spec-level decision is needed.
- Whether 视图's zoom and source/render toggles are enabled in the first implementation depends on existing APIs; if a toggle has no implementation, it is omitted from the menu rather than shown disabled.
