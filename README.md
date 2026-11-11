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
| `crates/typors-app` | Tiny loader `typors.exe` + the `dump` inspection binary |
| `crates/typors-spike` | M0 research record; independent regression reference |
| `assets/` | Runtime resources (icon, sample image) — not embedded in the EXE |
| `packaging/` | NSIS installer (`typors.nsi`) |

`typors.exe` loads `typors_runtime.dll` from its own directory. The DLL owns GPUI,
the editor and the first-run shell registration; the EXE only calls its
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
| `--dump-cursor` | Caret, hit testing, keyboard movement (C1–C6) |
| `--dump-edit` | Typing/deleting (E1–E4) |
| `--dump-ime` | Input-method callback sequence (I0–I8) |
| `--dump-select` | Selection, clipboard, undo (S1–S9) |
| `--dump-newlines` | Enter/newline handling (N0–N6) |
| `--dump-perf` | Frame-cost sweep (P0–P9) |

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
# -> packaging\dist\typors-0.1.0-setup.exe
```

The installer's optional sections (file association, desktop shortcut, Start-menu
shortcut) are **all unchecked**; the user chooses. The core files are the only
required section, and the Finish page's "run" box is unchecked too.

The old Inno Setup script (`packaging\typors.iss`) is kept only as history; NSIS
is the supported path.

## Environment variables

All are debug/probe hooks; none is needed to use the editor.

| Variable | Effect |
| --- | --- |
| `TYPORS_IME_LOG=off` | Silence the per-callback IME log on stderr |
| `TYPORS_EDIT_LOG=1` | One structured line per buffer change |
| `TYPORS_SCROLL=<px>` | Scroll the page to that document offset on the first frame |
| `TYPORS_CULL_LOG=1` | Print the culling band and shaped/culled fragment counts |
| `TYPORS_EXIT_DUMP=<ms>` | After that many ms, print buffer + caret and quit |

## Dependencies

`gpui-kit 0.8.0` (pins `gpui-pre 0.3.9`), `gpui-fps 0.8.0`, `pulldown-cmark
0.13.5`, `ropey 2.0.0-beta.1`, `ratex-* 0.1.15`, `merman 0.9.3`, `base64 0.22.1`,
`winreg 0.56.0`. Versions are pinned in the workspace `Cargo.toml`.

## Relation to `spike/`

`crates/typors-spike` is the M0 feasibility probe kept as a research record and
an independent regression reference. It does not depend on the product crates.
