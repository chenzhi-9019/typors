# Design

## Context

See `proposal.md` and the five delta specs. The current shell (from `add-menu-bar-and-fix-caret-input`) already has a menu row, file commands and a dirty revision; the caret is a single quad painted only while the editor has focus, and page metrics are hard-coded in `EditorTheme`. Settings do not exist. Typora's installed resources provide the exact target:

- `resources/style/base.css`: `--bg-color:#fff`, `--text-color:#333`, `--select-text-bg-color:#B5D6FC`, header 28 px, content top 28, footer 28 px.
- `resources/style/themes/github.css`: body `Open Sans 16px/1.6`, `#write max-width:860px`, `padding:30px`, bottom 100, 1024/1200 at 1400/1800 px.
- `resources/style/window.css` / `base-control.css`: menu top 26, item padding `3pt 24px 2pt 24px`, hover `rgba(229,229,229,.59)`, border `#e9e9e9`, modal shadow `0 15px 30px rgb(0 0 40 / 20%)`, preferences rail `#fafafa`, `--sidebar-width:270px`.

The current implementation also has a known limit: Typora is Chromium and typors is GPUI/Skia, so glyph antialiasing cannot be byte-identical. The enforceable contract is geometry, colors, font family/size/weight, spacing and control bounds; the visual harness treats glyph-edge differences as a separate report.

## Goals / Non-Goals

**Goals:**

- Blink like Typora and reset correctly on every activity.
- Minimal, high-value Markdown snippet completion with a Typora-styled popup.
- One close guard for every window-closing path.
- A persistent, Typora-styled preferences surface for all four selected groups.
- Replace ad-hoc page/chrome metrics with Typora's measured tokens.

**Non-Goals:**

- Language-server completion, word completion, path completion.
- Full Typora feature parity (sidebar, outline, export, themes marketplace, spell check).
- Pixel-identical glyph rasterization (see Context).
- Changing the document format or renders.

## Decisions

### 1. Settings live in a pure `typors-settings` crate

`Settings` is a serde struct with `version`, `appearance`, `files`, `editing`, `shortcuts`, `language`. Defaults equal Typora's light defaults. `Settings::load` reads `%APPDATA%\typors\settings.json`; writes go to `settings.json.tmp` then atomic rename; a corrupt file is renamed `.bak` and defaults are used with a non-blocking warning. Validation is `Settings::validate() -> Vec<FieldError>` and rejects out-of-range values; the UI shows field errors and disables 确定.

The crate depends only on serde/serde_json, so both runtime and tests can use it without GPUI. `EditorTheme` gains `from_settings(&Settings)` and a dark palette; `MarkdownView` layout takes `page_width` and `font` from the theme.

Alternative: put settings in runtime. Rejected: persistence/validation then cannot be unit-tested without GPUI, and `typors-render` needs the theme.

### 2. Blink controller in `MarkdownView`

Add `caret_visible: bool` and `blink: BlinkState { enabled, interval_ms, generation }` to `MarkdownView`. One `cx.spawn_in(window, ...)` loop per live view sleeps `interval`, flips `caret_visible` and calls `cx.notify()` only while the view has focus and no composition. `caret_visible` is forced true by `reset_blink()` from:

- `apply_edit_internal`, caret moves, selection changes, mouse down/drag, scroll, IME `replace_and_mark_text_in_range`, focus gained, menu close.
- `reset_blink` bumps a generation so an in-flight timer cannot overwrite a newer reset.

`paint` paints the caret only when `focus_handle.is_focused(window) && caret_visible`. The caret quad geometry is unchanged, so blinking cannot move text or scroll. A disabled setting paints whenever focused.

Alternative: a global timer in the shell. Rejected: it would need to know every editor action and would repaint even when the caret is not in an editor.

### 3. Completion is a pure catalog plus a shell popup

`typors-editor::completion` owns `CompletionState { range, prefix, items, selected }` and `suggest(doc, caret, trigger) -> Option<State>`. It is context-aware by block kind and line prefix and ranks exact/prefix matches. The catalog is the confirmed minimal set; no language server.

Trigger points:
- `insert_text_tagged` after a character edit, unless `marked.is_some()` or completion is disabled.
- A `cx.spawn` debounce of `settings.editing.completion_delay_ms`; a newer edit cancels the old task by generation.

The popup is rendered by `EditorShell` above the page using the caret's window-space anchor exposed by `MarkdownView::completion_anchor()`. The shell handles Up/Down/Enter/Tab/Esc and mouse; accepting calls `MarkdownView::accept_completion()`, which replaces the typed prefix in one `apply_edit_internal` call (one undo step). Opening/closing the popup never edits the buffer.

Popup geometry and colors come from the Typora visual tokens: white, 1 px `#e9e9e9`, shadow `0 1px 3px rgba(12,13,14,.1), 0 4px 20px rgba(12,13,14,.035)`, 28 px rows, 13 px text.

### 4. One close guard, synchronous native prompt

`EditorShell::confirm_close(window, cx) -> bool`:
1. If not dirty, return true.
2. If autosave has written the current revision, return true.
3. Show `MessageBoxW` with Save / Don't Save / Cancel.
4. Save: write synchronously; return false on failure and show the error.
5. Don't Save: true. Cancel: false.

`window.on_window_should_close(cx, move |window, cx| shell.update(...confirm_close...))` is installed in `open_window`. `文件 → 退出` calls the same method and then `cx.quit()`. The native dialog is synchronous, so it can answer the `bool` callback without blocking the platform close event.

The title is `"{name} - typors"` and adds `*` while `doc.revision() != saved_revision`; `EditorShell::render` updates the title when dirty changes.

### 5. Preferences is a shell modal with Typora metrics

`PreferencesState { open, category, draft: Settings, errors }` lives in `EditorShell`. It renders a full-window overlay:

- Backdrop `rgba(0,0,0,.1)` (Typora `modal-backdrop.in`).
- Panel centered, max 900×620, 1 px `#e9e9e9`, shadow `0 15px 30px rgb(0 0 40 / 20%)`.
- Left rail 240 px `#fafafa`; categories 32 px rows, 13 px text; active row `#eee` with a 2 px `#777` left border.
- Right pane `#fff`, 30 px padding, 13 px labels, 12 px `#999` hints, 28 px controls, 12 px gaps.
- Footer right-aligned `恢复默认值` left, `取消`/`确定` right; `确定` disabled on validation errors.

The draft is edited in memory; 取消 discards, 确定 applies and saves. `Esc` = cancel. Settings changes that affect layout call `view.update_settings(settings)` and relayout; theme changes swap `EditorTheme` and repaint. Language changes swap a `Lang` enum used by menu/settings/dialog strings; no restart.

Shortcut editing captures the next chord in the settings row and validates against the current map; conflicts show the existing command name. A read-only reference table is always shown.

Alternative: separate OS settings window. Rejected: Typora's preferences is a modal surface tied to the app, and a shell overlay keeps focus/undo state unified.

### 6. Typora tokens and page geometry

`EditorTheme` is split into `Palette` + typography metrics:

```text
bg #fff, text #333, muted #777, selection #B5D6FC, link #4183C4,
border #e9e9e9, hover rgba(229,229,229,.59), caret #333,
body Open Sans 16, line 1.6, mono "Lucida Console",Consolas,"Courier",monospace
```

Page layout: outer content area centered; `page_width = min(viewport - 0, settings.page_width)` with default 860, 1024/1200 at 1400/1800 px; content padding 30; page background white; header 28; status bar 28. `DocLayout` receives the already-computed content width so block geometry and hit testing stay unchanged. The status bar shows a source-mode toggle on the left and `N 词` on the right, 12 px, `#333` at 75% opacity.

### 7. Visual reference harness

`tools/visual-diff.ps1`:
- Launches Typora and typors with the same client size and a fixed blank/sample document.
- Captures client area (not the desktop), crops to the client rect, and writes PNGs under `docs/reference/typora/` and `docs/reference/typors/`.
- Compares exact background/border color at a sampled grid, control bounding boxes, and text bboxes; glyph antialiasing differences are reported in a separate mask and do not fail geometry.
- Fixtures: blank chrome, menu open, preferences (each category), completion popup, unsaved dialog.
- The script exits non-zero on a geometry or non-glyph color mismatch, and prints the first differing coordinates.

### 8. Testing and probes

- Unit: settings defaults/validation/round-trip/corrupt recovery; blink phase/reset math; completion catalog/ranking/context/accept single undo; dirty/title/three-way decision; shortcut conflict and language lookup.
- Render: theme token application, page width 860/1024/1200, caret blink visibility, popup anchor above/below.
- Probes: `--dump-blink` (phase timeline without a window), `--dump-completion` (trigger/ranking/accept), `--dump-settings` (defaults/validation/persistence), `--dump-close` (dirty decision matrix), `--dump-visual` (reference diff summary). Existing `--dump-*` remain green.
- Real-window checklist: blink, completion by keyboard/mouse, close X/Alt+F4, each settings category, language switch, theme switch, restart persistence.

## Risks / Trade-offs

- **Chromium vs Skia glyphs** → The harness separates antialiasing; document that pixel parity covers geometry/colors/fonts, not subpixel AA. Do not promise byte-identical text.
- **Settings-triggered relayout** → Page width/font size changes invalidate `DocLayout` and caches; call `view.update_settings` then `cx.notify`, and test caret/scroll preservation.
- **Blink timer battery/wakeups** → Timer only runs while focused and enabled; no frame work when hidden; stop when the entity is dropped.
- **Completion competing with IME/actions** → Never open during composition; popup keys only while open; Esc restores editor focus.
- **Close guard on OS shutdown** → `on_should_close` is synchronous; saving on shutdown may block. If the write fails, allow the OS to close but keep a recovery copy? Spec says veto; test the normal path and log shutdown failures.
- **Settings corrupt/migration** → Versioned schema, `.bak`, defaults; never block startup.
- **Shortcut remapping conflicts** → Validate against the full menu map and reject duplicates; keep the default table visible.

## Migration Plan

Settings file starts at version 1; absent → defaults. Future fields use `#[serde(default)]` so older files load. No document migration. Rollback: deleting `settings.json` returns to defaults; the visual theme changes are reversible by switching back to light.

## Open Questions

- Exact dark palette values are taken from Typora's `night.css` during implementation; if that file's variables differ from the minimum contrast checks, the spec's token contract wins.
- The first release may implement shortcut **rebinding for menu commands** only, not for arbitrary text-editing actions; this is recorded as a UI hint rather than silently pretending to support every chord.
