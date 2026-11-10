# typors

A hybrid Markdown editor: the document is drawn in its **render** projection
(headings without `#`, emphasis without `**`, list bullets instead of `- `),
while the one block that holds the caret is drawn as **Markdown source**. One
`ropey` buffer is the only truth; the two projections are views of it.

This crate is the product. `spike/` is the research probe the editor was proved
in, and it stays in the tree; see *Relation to `spike/`* below.

## Layout

| Path | What it is |
| --- | --- |
| `src/doc.rs` | The buffer: parse into blocks and spans, byte <-> render mapping |
| `src/render.rs` | Render-projection geometry: block boxes, fragments, byte <-> x, caches, viewport culling |
| `src/view.rs` | The self-drawn GPUI `Element`, the caret, the one text-mutation entry point |
| `src/ime.rs` | The Windows input-method channel: composition buffer, callback log, `--dump-ime` |
| `src/selection.rs` | Selection, highlight geometry, undo/redo, `--dump-select` |
| `src/perf.rs` | Frame-cost sweep and the geometry-equality checks (`--dump-perf`) |
| `src/probe.rs` | The programmatic caret / hit-test / typing probes (`--dump-cursor`, `--dump-edit`) |
| `src/sample.rs` | Deterministic sample-document generator (used by tests and probes) |
| `src/main.rs` | Modes: window, `--fps`, and the `--dump-*` probe channels |
| `src/bin/dump.rs` | `cargo run --release --bin dump -- sample.md`: the raw block/span table |
| `build.rs` | Embeds the Windows `VERSIONINFO` block and the icon |
| `assets/typors.ico` | Application icon (16/32/48/256), an original placeholder |
| `tools/make-icon.ps1` | Regenerates `assets/typors.ico`; `-PreviewPath` saves a PNG of the mark |
| `sample.md` | The document every dump mode is normally run against |

## Build

```powershell
cd typors
cargo build --release          # -> target\release\typors.exe
cargo test --release           # 30 unit tests
```

That executable is the installer payload; keep the path stable.

Windows only, in practice: the resource embedding in `build.rs` is a no-op on
other targets, but GPUI itself is what pins the platform support here.

Dependencies are deliberately fixed: `gpui-kit`, `gpui-fps`, `ropey`,
`pulldown-cmark`, plus `winresource` as a **build**-dependency. Nothing else is
added at runtime; `gpui-pre` is never declared directly (GPUI comes through the
Kit facade).

## Running

```powershell
cargo run --release -- sample.md                    # window: click, type, arrows
cargo run --release -- --fps sample.md              # ... with the gpui-fps HUD
```

### Probe modes

Each `--dump-*` mode opens the real window, paints one frame, then prints what
it measured and asserts its checks. **Exit code 0 means every check passed**;
a failing check prints `[fail] ...` and exits non-zero.

```powershell
cargo run --release -- --dump-layout  sample.md     # block boxes, painted fragments (A1-A5)
cargo run --release -- --dump-cursor  sample.md     # caret, hit testing, key handling (C1-C6)
cargo run --release -- --dump-cursor  4:0 sample.md # ... with the caret placed at block 4, byte 0
cargo run --release -- --dump-edit    sample.md     # typing/deleting (E1-E4)
cargo run --release -- --dump-ime     sample.md     # input-method callback sequence (I0-I8)
cargo run --release -- --dump-select  sample.md     # selection, clipboard, undo (S1-S9)
cargo run --release -- --dump-newlines sample.md    # Enter/newline handling and blank lines (N0-N1)
cargo run --release -- --dump-perf    sample.md     # frame-cost sweep (P0-P9)
```

`--dump-perf` sweeps 24 scroll positions plus typing at the end of the document;
on a 100k-character file it reports the min/median/p95/max of each relayout and
counts pauses over 100 ms. `--dump-layout` also prints per-block timing.

The probes are the regression evidence for the editor's behaviour. They are
part of the product binary on purpose: after a change, run them and compare.

### Environment variables

All of them are debug/probe hooks; none is needed to use the editor.

| Variable | Effect |
| --- | --- |
| `TYPORS_IME_LOG=off` (or `0`) | Silence the per-callback IME log on stderr |
| `TYPORS_EDIT_LOG=1` | One structured line per buffer change |
| `TYPORS_SCROLL=<px>` | Scroll the page to that document offset on the first frame |
| `TYPORS_CULL_LOG=1` | Print the culling band and shaped/culled fragment counts per frame |
| `TYPORS_EXIT_DUMP=<ms>` | After that many ms, print buffer + caret and quit (drives a real window from outside) |

## Windows resources

`build.rs` embeds, via `winresource`:

- the icon (`assets/typors.ico`) — file icon, taskbar, Alt-Tab;
- `VERSIONINFO`: `ProductName=typors`, `FileDescription=typors`,
  `FileVersion=0.1.0`, `ProductVersion=0.1.0`,
  `OriginalFilename=typors.exe`, `LegalCopyright=Copyright © 2026 typors`.

Check them with:

```powershell
(Get-Item typors\target\release\typors.exe).VersionInfo
```

The icon is an **original placeholder**: a geometric lower-case "t" on a navy
plate with a mint baseline caret, drawn by `tools/make-icon.ps1`. It is not
derived from any other product's icon, palette or typeface. Replacing it means
overwriting `assets/typors.ico` (16/32/48/256, 32-bit DIB) and rebuilding.

## Relation to `spike/`

`spike/` was the M0 feasibility probe: it answers whether the hybrid projection
is fast enough and whether the GPUI APIs can carry it. Its `findings.md` holds
the measurements and the API facts, and its test suite is the same 30 tests.

This crate is the same editor in a shipping shell: the modules were **copied**
from `spike/src/` unchanged, and only the crate name, the binary name, the
window title and the log prefix changed. No behaviour was touched.

The two copies therefore coexist for now — that is deliberate. `spike/` is kept
as the research record and as an independent regression reference (it builds and
tests on its own); converging them into one crate is M1 work, and doing it now
would invalidate the probe evidence before the product has its own baseline.

Consequence to keep in mind: a fix applied here must be applied there too until
the convergence happens.
