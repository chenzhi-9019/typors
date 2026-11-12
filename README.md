# typors

A hybrid Markdown editor: the document is drawn in its **render** projection
(headings without `#`, emphasis without `**`, list bullets instead of `- `), while
the one block that holds the caret is drawn as **Markdown source**. One `ropey`
buffer is the only truth; the two projections are views of it.

## Workspace layout

| Path | What it is |
| --- | --- |
| `crates/typors-core` | The buffer: parse into blocks and spans, byte <-> render mapping, sample generator |
| `crates/typors-media` | Image-source classification and the RaTeX math engine (no GPUI) |
| `crates/typors-render` | Render-projection geometry: block boxes, fragments, objects, tables, caches |
| `crates/typors-editor` | Self-drawn GPUI `Element`, caret, input/IME, selection, undo |
| `crates/typors-probes` | `--dump-*` probes and the frame-cost sweep |
| `crates/typors-runtime` | `cdylib`: window, modes, shell registration (the bulk of the program) |
| `crates/typors-app` | Tiny loader `typors.exe` |
| `assets/` | Runtime resources (icon, sample image) — not embedded in the EXE |
| `packaging/` | NSIS installer (`typors.nsi`) |

`typors.exe` loads `typors_runtime.dll` from its own directory. Mermaid lives in a
second lazy-loaded `typors_mermaid.dll`: the runtime DLL is ~16 MB and Mermaid's
28 MB only maps when a document actually contains a Mermaid fence. The DLL owns
GPUI, the editor and the first-run shell registration; the EXE only calls its
`typors_run` export. That is what keeps the EXE around 145 KB.

## Build

```powershell
cargo build --release --workspace      # builds typors.exe + typors_runtime.dll
cargo test --workspace
```

The executable and DLL must stay side by side:

```text
target\release\typors.exe
target\release\typors_runtime.dll
target\release\typors_mermaid.dll
```

## Running

```powershell
target\release\typors.exe sample.md
target\release\typors.exe --dump-layout sample.md
```

`--dump-*` modes open the real window, paint one frame, print what they measured
and assert their checks. Exit code 0 means every check passed.

| Mode | What it checks |
| --- | --- |
| `--dump-layout` | Block boxes, painted fragments, objects, A1–A5 |
| `--dump-media` | Image/math/table objects, source mapping, M1–M5 |
| `--dump-live-preview` | Heading/code marker hiding and rendered projection (L1–L3) |
| `--dump-cursor` | Caret, hit testing, keyboard movement (C1–C6) |
| `--dump-edit` | Typing/deleting (E1–E4) |
| `--dump-ime` | Input-method callback sequence (I0–I8) |
| `--dump-select` | Selection, clipboard, undo (S1–S9) |
| `--dump-newlines` | Enter/newline handling (N0–N6) |
| `--dump-perf` | Frame-cost sweep (P0–P9) |
| `--dump-blank` | Empty-document block/caret/typing/Enter contract (B1–B4); no file argument |
| `--dump-menu` | Menu model labels, item order and commands |
| `--dump-blink` | Caret blink phases, reset and disabled mode |
| `--dump-completion` | Markdown snippet catalog and no-match boundary |
| `--dump-settings` | Defaults, validation, save/load and corrupt recovery |
| `--dump-cache` | Cache root, directory mapping, entry round-trip, checksum and eviction |
| `--dump-session` | Reading-position store: record, reload, match, clamp and recovery |
| `--dump-chart` | Bar matrix (grouped, stacked, negative, horizontal, value labels) and the doc examples |
| `--cache-status` / `--clear-cache` | Report the cache root/size/entries, or empty it |

## Menu bar

The window has a GPUI-drawn menu row above the scrolling page:
`文件(F) / 编辑(E) / 段落(P) / 格式(O) / 视图(V) / 主题(T) / 帮助(H)`.

- Click a label or press `Alt+F/E/P/O/V/T/H` to open it.
- `←/→` switch menus, `↑/↓` move the highlight (disabled items are skipped),
  `Enter` runs, `Esc` closes and returns focus to the editor.
- 文件: New `Ctrl+N`, Open `Ctrl+O`, Save `Ctrl+S`, Save As `Ctrl+Shift+S`, Exit.
- 编辑 and 段落/格式 commands run through the same editor entry points as their
  keyboard shortcuts, so undo/redo, selection and IME stay consistent.

An empty startup (running `typors` without a file) is a real editable document:
there is one empty paragraph, the caret is at offset 0, and click/type/Enter/IME
work without a prior click.

## Rich content

- Images: local relative/absolute paths, `http(s)://` URLs and `data:` URIs, with
  placeholders while loading and a visible reason on failure.
- Math: inline `$...$` and display `$$...$$` through RaTeX, rendered as vector
  paths.
- GFM tables: grid, tinted header row, per-cell text; column alignment/colspan are
  not implemented yet.
- Strikethrough `~~...~~`.
- Inline HTML subset: `<b>/<strong>`, `<i>/<em>`, `<code>/<kbd>/<samp>`,
  `<s>/<del>`, `<a>` and `<br>`.
- **Charts**: fenced ```` ```chart ```` blocks render native SVG charts (pie, line,
  bar, scatter) with Matplotlib-style `series: values` and `series.option: value`
  definitions. Bar charts support grouped series, category labels (`labels`),
  value labels (`values`), a zero baseline for negatives, `horizontal` and
  `stacked`. [docs/chart.md](docs/chart.md) is the interface reference: grammar,
  every option, axis/legend rules, error messages and the change log.
- **Automatic TOC**: a standalone `[toc]` line expands to the document's heading
  outline at render time; the marker stays editable when the caret is on it.
- **Mermaid**: fenced ```` ```mermaid ```` blocks render natively through
  [merman](https://github.com/Latias94/merman), a headless Rust Mermaid
  implementation (no WebView, no Node). All Mermaid diagram families compiled by
  merman are enabled (flowchart, sequence, class, state, ER, gantt, pie, mindmap,
  …); diagrams are rasterized at the box size so Mermaid's own colours survive.
- **Block HTML**: tags are stripped and the common inline/block subset is mapped
  to text, emphasis, links and line breaks (`<h1>`–`<h6>`, `<p>`, `<ul>/<li>`,
  `<pre>`, `<br>`). There is no CSS box layout, so arbitrary `<div>` styling is not
  reproduced.
- **Still missing:** full HTML/CSS layout (a browser engine) if pixel-accurate HTML
  is required.

## First-run Markdown association

On the first normal launch the DLL asks whether to register typors as a Markdown
handler. It writes the per-user `HKCU\Software\Classes` entries and opens Windows
*Default Apps*; Windows 10/11 still require the user to confirm the default there
(the `UserChoice` hash is user-owned).

The installer can do it without the prompt:

```powershell
typors.exe --register
typors.exe --unregister
```

## Installer (NSIS)

```powershell
cargo build --release --workspace
pwsh packaging\build-nsis.ps1
# -> packaging\dist\typors-1.0.5-setup.exe
```

The installer's first choice is **install scope**: every user (Program Files +
HKLM) or only the current user (`%LOCALAPPDATA%` + HKCU). There is no default:
Next stays disabled until one radio button is picked. All optional sections (file
association, desktop shortcut, Start-menu shortcut) are **unchecked**; the user
chooses. The core files are the only required section, and the Finish page's
"run" box is unchecked too.

Silent installs must state the scope:

```powershell
typors-1.0.5-setup.exe /S /SCOPE=me /D=C:\path\to\typors
typors-1.0.5-setup.exe /S /SCOPE=all   # triggers UAC
```

The old Inno Setup script (`packaging\typors.iss`) is kept only as history; NSIS
is the supported path.

## Large documents: streaming render

A frame measures the visible band (plus margin) first and paints it, then spends
a bounded slice (24 blocks / 4 ms) measuring the nearest off-screen blocks into
the block cache and asks for another frame. Off-screen blocks keep an estimated
height, so the scrollbar is usable before the rest of the document is measured;
a block that becomes visible is measured before that frame paints it. Files over
256 KiB also open progressively: a safe prefix is parsed for the first frame and
a worker parses the rest, installing the whole-file block table when it arrives.

Edits are local: only the blank-line-separated unit that owns the edit (plus one
block of lookahead) is reparsed, with the whole-document parse as the fallback
and the oracle for tests. `TYPORS_EDIT_LOG=1` reports the blocks actually
reparsed.

## Scrolling

A scrollbar sits on the window's right edge and appears only when the page is
taller than the editor area; drag it (or click the track) to scroll. The caret is
always kept inside the visible editor band — the window minus the 28 px menu row
and status bar — so pressing Enter at the bottom scrolls just enough to show the
new line. The page itself stays centered at its configured width.

## Cache and reading positions

Rendered resources are cached under `<directory containing typors.exe>\cache`
(images as PNG bitmaps, chart/Mermaid rasters), bounded by
`files.cache_max_mb` (default 512 MB) with LRU eviction. When the executable
directory is not writable — an all-users install under Program Files — the cache
falls back to `%LOCALAPPDATA%\typors\cache`. The index maps each document's
canonical absolute path to its cache directory; a changed file invalidates only
its own directory. The cache is always safe to delete.

Each document's caret and scroll position are stored in
`%APPDATA%\typors\sessions.json` (independent of the cache) and restored when
the same unchanged file is opened again; a changed file is clamped rather than
guessed.

```powershell
typors.exe --cache-status     # root, size, per-document entries
typors.exe --clear-cache      # remove cached data; documents are untouched
```

## Environment variables

All are debug/probe hooks; none is needed to use the editor.

| Variable | Effect |
| --- | --- |
| `TYPORS_IME_LOG=off` | Silence the per-callback IME log on stderr |
| `TYPORS_EDIT_LOG=1` | One structured line per buffer change |
| `TYPORS_SCROLL=<px>` | Scroll the page to that document offset on the first frame |
| `TYPORS_CULL_LOG=1` | Print the culling band and shaped/culled fragment counts |
| `TYPORS_EXIT_DUMP=<ms>` | After that many ms, print buffer + caret and quit |
| `TYPORS_STREAMING=off` | Disable viewport-first layout (whole-document layout, the pre-change path) |
| `TYPORS_CACHE=off` | Disable the persistent render cache for this session |
| `TYPORS_CACHE_DIR=<dir>` | Use this cache root instead of the exe directory/fallback |
| `TYPORS_SESSIONS_PATH=<file>` | Use this reading-position store instead of `%APPDATA%\typors\sessions.json` |

## Dependencies

`gpui-kit 0.8.0` (pins `gpui-pre 0.3.9`), `gpui-fps 0.8.0`, `pulldown-cmark
0.13.5`, `ropey 2.0.0-beta.1`, `ratex-* 0.1.15`, `merman 0.9.3`, `base64 0.22.1`,
`winreg 0.56.0`. Versions are pinned in the workspace `Cargo.toml`.

## History

The M0 feasibility spike was a separate `typors-spike` crate during development.
It has been removed: `typors` now has one executable and the probes in
`crates/typors-probes` cover the regression surface the spike used to guard.

## Live preview

Typing `# ` / `## ` / `- ` / `1. ` / `> ` hides the marker immediately and keeps
the line rendered while the caret stays in it. An opening fence line stays as
plain text until Enter terminates it, so typing three backticks (or a partial
language) never replaces the line with an empty code plate mid-word; once the
line is complete the plate renders while editing. `chart` / `mermaid` fences stay
as source while the caret is inside and render only after the caret leaves. The
caret mapping and A5/C6/E/S/I probes were updated for this contract; inline
emphasis markers still always hide (caret-local reveal is tracked separately).

While the opening fence is being typed, the completion popup lists fence
languages (filtered by the partial word; `↑`/`↓` moves, `Enter`/`Tab` accepts and
opens the block, `Esc` closes). Headings (`#`) and inline-code backticks do not
open the snippet popup.

With the caret inside a fenced block, a language chip sits at the block's
top-right. Click it to edit the language in place: the chip becomes a filter
box over the same language list (typing replaces the current value, `Backspace`
edits it, `Enter`/`Tab` commits the selected language to the fence line,
`Esc` or a click in the page cancels). Changing the language keeps the block
body and the caret position.
