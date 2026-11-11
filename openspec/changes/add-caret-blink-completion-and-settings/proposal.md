# Proposal

## Why

The current editor has a caret that never blinks, no Markdown authoring hints, no save prompt when the window is closed with the X button, and no settings surface at all. The menu and page also do not match Typora: Typora uses a 28 px header, a 26 px menu row, an 860 px page at 16 px Open Sans with 30 px padding, `#333` text on `#fff`, and a 28 px status bar, while typors uses different type metrics and no status bar. The user wants the new work to be visually indistinguishable from Typora, pixel for pixel, at the reference size.

## What Changes

- **Caret blink**: standard on/off blink (530 ms visible / 530 ms hidden) while the editor is focused; typing, clicking, moving, selecting, IME composition and menu focus reset it to visible; hidden when unfocused; configurable in settings.
- **Markdown completion**: a Typora-style inline suggest popup for the most-needed Markdown constructs only — headings, lists, quote, fenced code, bold/italic/strikethrough, inline code, inline/display math, link, image, table and rule. Keyboard (`↑/↓/Enter/Tab/Esc`) and mouse selection; no completion during IME composition; toggled/delayed in settings.
- **Unsaved-changes guard**: `文件 → 退出`, the window close button, `Alt+F4` and OS shutdown all route through the same Save / Don't Save / Cancel dialog; closing is vetoed on Cancel and on a failed save. The window title carries Typora's modified marker (`*`).
- **Preferences**: a Typora-style settings surface (left category rail, right scrolling pane, footer buttons) covering:
  - 外观：theme (light/dark/system), font family, font size, line height, page width, live preview.
  - 文件与保存：autosave, prompt on close, restore last session/file, default save directory.
  - 编辑：completion on/off and delay, caret blink on/off and interval, bracket/quote pairing.
  - 快捷键与语言：shortcut reference table, editable menu shortcuts, UI language (简体中文 / English / follow system).
- **Typora pixel parity**: replace ad-hoc metrics with tokens measured from the installed Typora 1.x default theme and CSS:
  - `--bg-color:#fff`, `--text-color:#333`, `--select-text-bg-color:#B5D6FC`, window border `#e9e9e9`, hover `rgba(229,229,229,.59)`, link `#4183C4`.
  - Header 28 px; menu row top 26 px, item font 13 px; page `max-width:860px`, centered, padding `30px`, `padding-bottom:100px`; body `Open Sans 16px/1.6`; headings `2rem/1.8rem/1.6rem/1.4rem/1.2rem/1rem`; code `0.9rem`, monospace stack `"Lucida Console",Consolas,"Courier",monospace`.
  - Status bar 28 px, 12 px text, left outline/source icons, right word count.
  - Menu dropdowns, settings panes, completion popup and save dialog reuse Typora's colors, borders, radii, shadows and spacing from the shipped CSS.
- Add a visual regression harness that captures typors and Typora at the same 1047×689 client size and diffs the chrome, page and settings surfaces.

## Capabilities

### New Capabilities

- `editor/typora-visual-parity`: measured Typora tokens, page geometry, chrome/status bar, visual fixtures and pixel-diff acceptance.
- `editor/caret-blink`: blink timing, reset rules, focus/IME interactions and settings.
- `editor/markdown-completion`: trigger/context, ranked snippets, popup behavior, insertion and undo.
- `editor/unsaved-changes`: dirty tracking, close interception, Save/Don't Save/Cancel and failure handling.
- `editor/preferences`: settings model, persistence, categories, validation and live application.

## Impact

- `crates/typors-editor`：caret blink state/timer, completion state and popup painting, page width/font/theme tokens, status-bar inputs.
- `crates/typors-runtime`：window close interception, settings window, menu/shortcut registration, save guard integration.
- `crates/typors-render`：`EditorTheme` becomes a serializable settings-driven token set (light/dark), page max-width and Typora metrics.
- `crates/typors-core`：no document-format change; dirty revision remains in `MarkdownView`.
- New `crates/typors-settings` (or runtime module): schema, defaults, JSON persistence, migration and validation.
- `packaging/`：ship default settings/theme assets; no installer behavior change.
- Tests/probes: `--dump-blink`, `--dump-completion`, `--dump-settings`, visual fixture comparison; existing probes must stay green.

## Assumptions

- “代码提示” means **Markdown syntax snippets**, as confirmed; no language-server completion, word completion or path completion in this change.
- “设置” covers all four selected groups; the first implementation may ship a read-only shortcut table plus editable menu shortcuts, but no arbitrary key-sequence macro editor.
- Pixel parity is targeted at Typora's default light theme at 100% scale and the 1047×689 reference; dark theme follows Typora's `night.css` tokens. Text antialiasing can differ because Typora renders with Chromium and typors with GPUI/Skia; geometry, colors, font family/size/weight and spacing are the enforceable pixel contract.
