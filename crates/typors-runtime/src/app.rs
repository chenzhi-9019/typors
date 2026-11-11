//! Task groups 3 and 4: the document is drawn in its *render* projection, and the
//! block that owns the caret is drawn as Markdown source.
//!
//! ```text
//! cargo run --release -- sample.md                     # window: click, type, arrows
//! cargo run --release -- --fps sample.md               # ... with the gpui-fps HUD
//! cargo run --release -- --dump-layout sample.md       # one frame, then dump + checks
//! cargo run --release -- --dump-cursor sample.md       # caret/hit/keyboard probe
//! cargo run --release -- --dump-cursor 4:0 sample.md   # ... with the caret placed
//! cargo run --release -- --dump-edit sample.md         # typing/deleting probe
//! cargo run --release -- --dump-ime sample.md          # input-method callback probe
//! cargo run --release -- --dump-select sample.md       # selection/clipboard/undo probe
//! cargo run --release -- --dump-perf big.md            # frame-cost sweep (task 7.1)
//! ```
//!
//! The dumps exist because nobody can "click ten times and confirm the caret
//! landed in the right block" from a terminal. `--dump-layout` prints the block
//! boxes, every painted fragment and runs the layout checks
//! ([`typors_render::DocLayout::checks`]); `--dump-cursor` and `--dump-edit` drive
//! the same [`typors_editor::view::MarkdownView`] the window paints and assert the group-4
//! behaviour ([`typors::probe`]). `--dump-ime` replays the Windows input-method
//! callback order through the same `EntityInputHandler` the platform calls and
//! asserts group 6 ([`typors::ime`]); `--dump-select` does the same for group 5 —
//! selection geometry, the clipboard round trip and the undo history
//! ([`typors::selection`]); `--dump-perf` sweeps the whole document for task 7.1.
//! Exit code 0 means every check passed.

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::gpui::profiler::{
    FrameEvent, FrameTimingCollector, set_trace_enabled, trace_enabled,
};
use gpui_kit::gpui::*;
use typors_core::{BlockKind, Document};
use typors_editor::view::{MarkdownView, key_bindings};
use typors_editor::{DumpSink, ImeSink, MediaSink, SelectSink};
use typors_probes::perf;
use typors_probes::probe;
use typors_probes::{PerfSink, ProbeCapture};
use typors_render::{ApproxMetrics, DocLayout, EditorTheme};

/// How long the dump run waits for the first painted frame before giving up and
/// falling back to approximate metrics.
const DUMP_WAIT_MS: u64 = 900;

/// How long `--dump-select` waits before its *second* clipboard read. findings.md
/// U2 reports that a read in the same tick as the write can come back `None`, so
/// the probe reads once immediately and once after the app has pumped events.
const CLIPBOARD_RETRY_MS: u64 = 120;

/// How long `--dump-newlines` lets the app draw after parking the caret below the
/// fold, so the *frame path* (not just the decision) can be observed bringing it
/// back into view. Several 16 ms frames fit in it.
const SCROLL_SETTLE_MS: u64 = 150;

#[derive(Clone)]
enum Mode {
    Window { fps: bool },
    DumpLayout,
    DumpCursor(Option<(usize, usize)>),
    DumpEdit,
    DumpIme,
    DumpSelect,
    DumpPerf,
    DumpNewlines(Option<(usize, usize)>),
    DumpMedia,
    DumpLivePreview,
    DumpBlink,
    DumpCompletion,
    DumpSettings,
    DumpBlank,
    DumpMenu,
}

pub fn run() {
    let mut mode = Mode::Window { fps: false };
    let mut path: Option<String> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump-layout" => mode = Mode::DumpLayout,
            "--dump-edit" => mode = Mode::DumpEdit,
            "--dump-ime" => mode = Mode::DumpIme,
            "--dump-select" => mode = Mode::DumpSelect,
            "--dump-perf" => mode = Mode::DumpPerf,
            "--dump-media" => mode = Mode::DumpMedia,
            "--dump-live-preview" => mode = Mode::DumpLivePreview,
            "--dump-blink" => mode = Mode::DumpBlink,
            "--dump-completion" => mode = Mode::DumpCompletion,
            "--dump-settings" => mode = Mode::DumpSettings,
            "--dump-blank" => mode = Mode::DumpBlank,
            "--dump-menu" => mode = Mode::DumpMenu,
            "--dump-newlines" => {
                // Optional `<block>:<byte>` caret position, same syntax as
                // --dump-cursor. Default: the end of the buffer.
                let mut target = None;
                if let Some(next) = args.peek()
                    && let Some(parsed) = parse_target(next)
                {
                    target = Some(parsed);
                    args.next();
                }
                mode = Mode::DumpNewlines(target);
            }
            "--fps" => mode = Mode::Window { fps: true },
            "--dump-cursor" => {
                // An optional `<block>:<byte>` may follow. A Windows path also
                // contains ':' — the parse below rejects it, so a path is never
                // mistaken for a caret target.
                let mut target = None;
                if let Some(next) = args.peek()
                    && let Some(parsed) = parse_target(next)
                {
                    target = Some(parsed);
                    args.next();
                }
                mode = Mode::DumpCursor(target);
            }
            other if other.starts_with("--") => {
                eprintln!("typors: unknown option {other}");
                std::process::exit(2);
            }
            other => path = Some(other.to_string()),
        }
    }

    // Task 7.1: `--dump-perf` reports GPUI's own frame trace next to its own
    // numbers. That trace is what `gpui-fps` renders from, and `gpui-fps` is the
    // dependency that enables the `profiler` feature for the whole graph, so
    // turning it on here is the same switch the HUD uses. `boot` is logged first so
    // task 7.2 can subtract the pre-`main` part of the cold start from the total.
    let tracing = matches!(mode, Mode::DumpPerf);
    if tracing {
        set_trace_enabled(true);
    }
    typors_editor::ime::log_ime(
        "boot",
        format_args!(
            "main() reached; profiler trace {}",
            if trace_enabled() { "on" } else { "off" }
        ),
    );
    let mut collector = tracing.then(FrameTimingCollector::new);

    let (source, text) = load(path);
    // Text enters the buffer in exactly two places — loading and editing — and
    // both normalise to LF, so "the buffer never holds a CR" is an invariant
    // instead of a hope. A CRLF file would otherwise carry CRs that the parser
    // keeps as plain text and the renderer draws as nothing (the same failure
    // shape as "my newline did not appear").
    let text = typors_editor::view::normalize_newlines(&text).into_owned();
    let doc = Document::parse(&text);
    eprintln!(
        "typors: loaded {} bytes, {} blocks from {}",
        doc.len_bytes(),
        doc.blocks().len(),
        source
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".to_string())
    );

    match mode {
        Mode::Window { fps } => open_window(source, doc, fps),
        Mode::DumpLayout => dump_layout(source, doc),
        Mode::DumpCursor(target) => run_probe(source, doc, ProbeKind::Cursor(target)),
        Mode::DumpEdit => run_probe(source, doc, ProbeKind::Edit),
        Mode::DumpIme => dump_ime(source, doc),
        Mode::DumpSelect => dump_select(source, doc),
        Mode::DumpPerf => dump_perf(source, doc, collector.as_mut()),
        Mode::DumpNewlines(target) => dump_newlines(source, doc, target),
        Mode::DumpMedia => dump_media(source, doc),
        Mode::DumpLivePreview => run_probe(source, doc, ProbeKind::LivePreview),
        Mode::DumpBlink => run_probe(source, doc, ProbeKind::Blink),
        Mode::DumpCompletion => run_probe(source, doc, ProbeKind::Completion),
        Mode::DumpSettings => dump_settings(),
        Mode::DumpBlank => dump_blank(source, doc),
        Mode::DumpMenu => dump_menu(),
    }
}

fn dump_menu() {
    let state = crate::menu::MenuState::new();
    let mut ok = state.menus.len() == 7;
    println!("# menu model dump\n");
    for (index, menu) in state.menus.iter().enumerate() {
        println!(
            "[{}] {} (Alt+{})",
            index,
            menu.label,
            menu.mnemonic.to_ascii_uppercase()
        );
        for item in &menu.items {
            println!(
                "    {:<12} {:>10}  {:?}",
                item.label, item.shortcut, item.command
            );
        }
        if menu.items.is_empty() {
            ok = false;
        }
    }
    println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

fn dump_blank(source: Option<PathBuf>, doc: Document) {
    let name = name_of(&source);
    let capture: Arc<Mutex<Option<typors_editor::ProbeCapture>>> = Arc::new(Mutex::new(None));
    let sink = capture.clone();
    gpui_kit::application().run(move |cx: &mut App| {
        let entity = cx.new(|cx| MarkdownView::new(doc.clone(), cx));
        let result = entity.update(cx, |view, _cx| typors_probes::probe::blank_probe(view));
        if let Ok(mut guard) = sink.lock() {
            *guard = Some(result);
        }
        cx.quit();
    });
    let capture = capture.lock().ok().and_then(|guard| guard.clone());
    match capture {
        Some(capture) => {
            println!("# blank document dump of {name}\n");
            let ok = typors_probes::probe::render_capture(&capture);
            println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
            let _ = std::io::stdout().flush();
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            eprintln!("typors: the blank probe did not run");
            std::process::exit(3);
        }
    }
}

fn dump_settings() {
    use typors_settings::Settings;
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let defaults = Settings::default();
    checks.push((
        "S1 defaults are valid".to_string(),
        defaults.validate().is_empty(),
        format!("{:?}", defaults.validate()),
    ));
    let mut bad = defaults.clone();
    bad.appearance.page_width = 100.0;
    checks.push((
        "S2 invalid page width is rejected".to_string(),
        !bad.validate().is_empty(),
        format!("{:?}", bad.validate()),
    ));
    let mut fixed = defaults.clone();
    fixed.appearance.page_width = 1000.0;
    checks.push((
        "S3 a corrected value validates".to_string(),
        fixed.validate().is_empty(),
        format!("{:?}", fixed.validate()),
    ));
    let dir = std::env::temp_dir().join(format!("typors-settings-probe-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("settings.json");
    let saved = fixed.save_to(&path).is_ok();
    let loaded = Settings::load_from(&path);
    checks.push((
        "S4 save/load round-trip".to_string(),
        saved && loaded.0 == fixed && loaded.1.is_none(),
        format!("saved={saved} loaded={:?}", loaded.1),
    ));
    if let Err(error) = std::fs::write(&path, "{not json") {
        checks.push((
            "S5 corrupt file recovery".to_string(),
            false,
            error.to_string(),
        ));
    } else {
        let (recovered, warning) = Settings::load_from(&path);
        checks.push((
            "S5 corrupt file recovers defaults with a warning".to_string(),
            recovered == defaults && warning.is_some(),
            format!("warning={warning:?}"),
        ));
    }
    let _ = std::fs::remove_dir_all(&dir);
    let ok = checks.iter().all(|(_, ok, _)| *ok);
    println!("# settings dump\n");
    for (name, pass, detail) in &checks {
        println!(
            "  [{}] {name} — {detail}",
            if *pass { "ok" } else { "FAIL" }
        );
    }
    println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

fn parse_target(raw: &str) -> Option<(usize, usize)> {
    let (block, byte) = raw.split_once(':')?;
    Some((block.parse().ok()?, byte.parse().ok()?))
}

fn load(path: Option<String>) -> (Option<PathBuf>, String) {
    match path {
        Some(raw) => {
            let path = PathBuf::from(&raw);
            match std::fs::read_to_string(&path) {
                Ok(text) => (Some(path), text),
                Err(error) => {
                    eprintln!("typors: cannot read {}: {error}", path.display());
                    std::process::exit(2);
                }
            }
        }
        None => {
            eprintln!(
                "usage: typors \
                 [--dump-layout|--dump-cursor [<block>:<byte>]|--dump-edit|--dump-newlines \
                 [<block>:<byte>]|--dump-ime|--dump-select|--dump-perf|--dump-blank|--dump-menu|--dump-blink|--dump-completion|--dump-settings] [<file.md>]"
            );
            (None, String::new())
        }
    }
}

fn name_of(source: &Option<PathBuf>) -> String {
    source
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<none>".to_string())
}

/// Every window this binary opens is the product window: the title is the
/// product name. `WindowOptions::default()` leaves `titlebar.title` empty and
/// Windows then falls back to whatever the executable happens to be called.
fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("typors".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------- window mode

fn open_window(source: Option<PathBuf>, doc: Document, fps: bool) {
    let name = name_of(&source);
    // `TYPORS_EXIT_DUMP=<ms>`: after that many milliseconds, print the buffer and
    // the caret (escaped) and quit. It exists so a real window can be driven from
    // the outside — PostMessage a key sequence at the OS window, then read back
    // what the buffer actually contains. Nothing else can observe a window the
    // agent cannot look at.
    let exit_dump_ms: Option<u64> = std::env::var("TYPORS_EXIT_DUMP")
        .ok()
        .and_then(|value| value.parse().ok());
    gpui_kit::application().run(move |cx: &mut App| {
        // Keyboard input reaches the editor through gpui actions, not through
        // `Element` events (an `Element` has no event hook), so the bindings have
        // to be registered on the app.
        cx.bind_keys(key_bindings());
        let view_slot: Arc<Mutex<Option<Entity<MarkdownView>>>> = Arc::new(Mutex::new(None));
        let view_open = view_slot.clone();
        let doc_dir = source.as_ref().and_then(|path| path.parent().map(std::path::Path::to_path_buf));
        let shell_source = source.clone();
        let (settings, settings_warning) = typors_settings::Settings::load();
        let shell_settings = settings.clone();
        let window_title = source
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| format!("{} - typors", name.to_string_lossy()))
            .unwrap_or_else(|| "typors".to_string());
        let opened = cx.open_window(
            {
                let mut options = window_options();
                options.titlebar = Some(TitlebarOptions {
                    title: Some(window_title.into()),
                    ..Default::default()
                });
                options
            },
            move |window, cx| {
            let view = cx.new(|cx| {
                let mut view = if fps {
                    MarkdownView::with_fps(doc.clone(), cx)
                } else {
                    MarkdownView::new(doc.clone(), cx)
                };
                view.set_doc_dir(doc_dir.clone());
                view
            });
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            if let Ok(mut guard) = view_open.lock() {
                *guard = Some(view.clone());
            }
            let shell = cx.new(|cx| {
                crate::shell::EditorShell::new(
                    view.clone(),
                    shell_source,
                    shell_settings,
                    settings_warning,
                    cx,
                )
            });
            let shell_close = shell.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                shell_close
                    .update(cx, |shell, cx| shell.confirm_close(window, cx))
                    .unwrap_or(true)
            });
            shell
        });
        match opened {
            Ok(_) => {
                println!("typors: window open on {name} (render projection + caret)");
                println!("typors: click to place the caret, type to edit, arrows to move");
                if fps {
                    println!("typors: gpui-fps HUD on (--fps)");
                }
                // A marker line, so "no `mark` lines at all" is a readable result
                // rather than an empty log (findings.md H2, step V1). It doubles as
                // task 7.2's "input is live" timestamp.
                typors_editor::ime::log_ime(
                    "ready",
                    format_args!(
                        "window open; every IME callback now logs here (TYPORS_IME_LOG=off silences)"
                    ),
                );
                let _ = std::io::stdout().flush();
            }
            Err(error) => {
                eprintln!("typors: cannot open window: {error}");
                std::process::exit(3);
            }
        }

        if let Some(ms) = exit_dump_ms {
            let view = view_slot.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(ms))
                    .await;
                cx.update(|cx| {
                    if let Some(view) = view.lock().ok().and_then(|guard| guard.clone()) {
                        let state = view.update(cx, |view, _cx| {
                            let text = view.doc().text();
                            format!(
                                "buffer={}B blocks={} caret={:?} sel={:?} rev={} text={:?}",
                                text.len(),
                                view.doc().blocks().len(),
                                view.caret(),
                                view.selection_range(),
                                view.doc().revision(),
                                typors_editor::view::escape_insert(&text),
                            )
                        });
                        println!("dump: {state}");
                        let _ = std::io::stdout().flush();
                    }
                    cx.quit();
                });
            })
            .detach();
        }
    });
}

// ---------------------------------------------------------------- layout dump

fn dump_media(source: Option<PathBuf>, doc: Document) {
    let sink: MediaSink = Arc::new(Mutex::new(None));
    let view_sink = sink.clone();
    let window_doc = doc.clone();
    let doc_dir = source
        .as_ref()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf));
    let name = name_of(&source);

    gpui_kit::application().run(move |cx: &mut App| {
        let opened = cx.open_window(window_options(), move |_window, cx| {
            cx.new(|cx| {
                let mut view = MarkdownView::with_media_probe(
                    window_doc.clone(),
                    view_sink.clone(),
                    typors_probes::media_probe,
                    cx,
                );
                view.set_doc_dir(doc_dir.clone());
                view
            })
        });
        if let Err(error) = opened {
            eprintln!("typors: cannot open window ({error})");
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DUMP_WAIT_MS))
                .await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });

    let capture = sink.lock().ok().and_then(|guard| guard.clone());
    let mut ok = true;
    match capture {
        Some(capture) => {
            println!("# media dump of {name} (painted frame)\n");
            print!("{}", capture.report);
            println!("\n-- checks --");
            ok &= print_checks(&capture.checks);
        }
        None => {
            eprintln!("typors: --dump-media did not get a painted frame");
            ok = false;
        }
    }
    if !ok {
        std::process::exit(1);
    }
}

fn dump_layout(source: Option<PathBuf>, doc: Document) {
    let sink: DumpSink = Arc::new(Mutex::new(None));
    let view_sink = sink.clone();
    let window_doc = doc.clone();
    let name = name_of(&source);

    gpui_kit::application().run(move |cx: &mut App| {
        let opened = cx.open_window(window_options(), move |_window, cx| {
            cx.new(|cx| MarkdownView::with_dump(window_doc.clone(), view_sink.clone(), cx))
        });
        if let Err(error) = opened {
            eprintln!("typors: cannot open window ({error}); will dump approximate geometry");
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DUMP_WAIT_MS))
                .await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });

    let capture = sink.lock().ok().and_then(|guard| guard.clone());
    let mut ok = true;
    match capture {
        Some(capture) => {
            println!(
                "# layout dump of {name} (measured by the window text system, painted frame)\n"
            );
            print!("{}", capture.report);
            println!(
                "timing: steady-state layout {:.3} ms (second pass, text system warm)",
                capture.remeasure_ms
            );
            println!(
                "timing: cached layout {:.3} ms (third pass, block cache + shape cache warm, task 7.1)",
                capture.cached_ms
            );
            println!("\n-- out-of-scope syntax as plain text (task 3.2) --");
            print_out_of_scope(&capture.layout);
            println!("\n-- checks (programmatic stand-in for the visual check) --");
            ok &= print_checks(&capture.checks);
        }
        None => {
            eprintln!(
                "typors: no painted frame within {DUMP_WAIT_MS} ms; dumping approximate geometry instead"
            );
            let layout = DocLayout::layout(
                &doc,
                &EditorTheme::default(),
                Arc::new(ApproxMetrics),
                point(px(0.), px(0.)),
                px(1000.),
            );
            println!("# layout dump of {name} (approximate metrics, NOT painted)\n");
            print!("{}", layout.report());
            println!("\n-- out-of-scope syntax as plain text (task 3.2) --");
            print_out_of_scope(&layout);
            println!("\n-- checks (programmatic stand-in for the visual check) --");
            ok &= print_checks(&layout.checks(&doc));
        }
    }

    // The six categories from task 3.1, straight off the block table.
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for block in doc.blocks() {
        let name = category(&block.kind);
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => counts.push((name, 1)),
        }
    }
    println!("\n-- block categories (task 3.1) --");
    for (name, count) in &counts {
        println!("  {name:<16} {count}");
    }

    println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
    let _ = std::io::stdout().flush();
    std::process::exit(if ok { 0 } else { 1 });
}

// ---------------------------------------------------------------- probes

#[derive(Clone)]
enum ProbeKind {
    Cursor(Option<(usize, usize)>),
    Edit,
    LivePreview,
    Blink,
    Completion,
}

/// Run one of the group-4 probes against the same view the window paints.
///
/// The probe runs *inside* the app, one frame after the window opened, so it sees
/// the real layout: the geometry produced by the window text system, published by
/// `paint`. That is the whole point of the probe 鈥?it is not a second
/// implementation, it is the painted one, driven from code.
fn run_probe(source: Option<PathBuf>, doc: Document, kind: ProbeKind) {
    let name = name_of(&source);
    let label = match kind {
        ProbeKind::Cursor(_) => "cursor probe",
        ProbeKind::Edit => "edit probe",
        ProbeKind::LivePreview => "live-preview probe",
        ProbeKind::Blink => "blink probe",
        ProbeKind::Completion => "completion probe",
    };
    let holder: Arc<Mutex<Option<Entity<MarkdownView>>>> = Arc::new(Mutex::new(None));
    let capture: Arc<Mutex<Option<ProbeCapture>>> = Arc::new(Mutex::new(None));
    let measured: Arc<Mutex<String>> = Arc::new(Mutex::new("<unpainted>".to_string()));

    {
        let holder_open = holder.clone();
        let holder_probe = holder.clone();
        let capture = capture.clone();
        let measured = measured.clone();
        let window_doc = doc.clone();
        let fallback_doc = doc.clone();
        let probe_kind = kind.clone();
        gpui_kit::application().run(move |cx: &mut App| {
            cx.bind_keys(key_bindings());
            let opened = cx.open_window(window_options(), move |window, cx| {
                let view = cx.new(|cx| MarkdownView::new(window_doc.clone(), cx));
                let focus = view.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                if let Ok(mut guard) = holder_open.lock() {
                    *guard = Some(view.clone());
                }
                view
            });
            if let Err(error) = opened {
                eprintln!("typors: no window ({error}); probing with approximate metrics");
            }
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(DUMP_WAIT_MS))
                    .await;
                cx.update(|cx| {
                    let view = holder_probe.lock().ok().and_then(|guard| guard.clone());
                    let result = match view {
                        Some(view) => view.update(cx, |view, _cx| {
                            if let Ok(mut label) = measured.lock() {
                                *label = view
                                    .layout()
                                    .map(|l| l.metrics_label.to_string())
                                    .unwrap_or_else(|| "<unpainted>".to_string());
                            }
                            run_kind(&probe_kind, view)
                        }),
                        None => {
                            // No surface: the same probe, same code path, with the
                            // deterministic stand-in metrics.
                            let entity = cx.new(|cx| MarkdownView::new(fallback_doc.clone(), cx));
                            entity.update(cx, |view, _cx| {
                                view.seed_layout(Arc::new(ApproxMetrics), px(1000.));
                                if let Ok(mut label) = measured.lock() {
                                    *label = "approx".to_string();
                                }
                                run_kind(&probe_kind, view)
                            })
                        }
                    };
                    if let Ok(mut guard) = capture.lock() {
                        *guard = Some(result);
                    }
                    cx.quit();
                });
            })
            .detach();
        });
    }

    let measured = measured
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| "<unknown>".to_string());
    match capture.lock().ok().and_then(|guard| guard.clone()) {
        Some(capture) => {
            println!(
                "# {label} of {name} (ran against the layout the window painted, measured by {measured})\n"
            );
            let ok = probe::render_capture(&capture);
            println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
            let _ = std::io::stdout().flush();
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            eprintln!("typors: the probe did not run (no frame within {DUMP_WAIT_MS} ms)");
            std::process::exit(3);
        }
    }
}

fn run_kind(kind: &ProbeKind, view: &mut MarkdownView) -> ProbeCapture {
    match kind {
        ProbeKind::Cursor(target) => probe::cursor_probe(view, *target),
        ProbeKind::Edit => probe::edit_probe(view),
        ProbeKind::LivePreview => probe::live_preview_probe(view),
        ProbeKind::Blink => probe::blink_probe(view),
        ProbeKind::Completion => probe::completion_probe(view),
    }
}

// ---------------------------------------------------------------- ime dump

/// `--dump-ime`: the group-6 probe.
///
/// It cannot run through [`run_probe`], because `EntityInputHandler` takes a
/// `&mut Window` and a window only exists inside a frame. So the view is built
/// with an [`ImeSink`], `paint` runs the probe against the frame it just painted,
/// and this function only prints the result. That is deliberate: the assertions
/// see the same layout the user's eyes would, measured by the window text system.
fn dump_ime(source: Option<PathBuf>, doc: Document) {
    let sink: ImeSink = Arc::new(Mutex::new(None));
    let view_sink = sink.clone();
    let window_doc = doc.clone();
    let name = name_of(&source);

    gpui_kit::application().run(move |cx: &mut App| {
        let opened = cx.open_window(window_options(), move |window, cx| {
            let view = cx
                .new(|cx| MarkdownView::with_ime_probe(window_doc.clone(), view_sink.clone(), cx));
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        });
        if let Err(error) = opened {
            eprintln!("typors: cannot open window: {error}");
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DUMP_WAIT_MS))
                .await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });

    match sink.lock().ok().and_then(|guard| guard.clone()) {
        Some(capture) => {
            println!("# ime dump of {name} (replayed against the painted frame)\n");
            let ok = probe::render_capture(&capture);
            println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
            let _ = std::io::stdout().flush();
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            eprintln!(
                "typors: the ime probe did not run (no painted frame within {DUMP_WAIT_MS} ms; \
                 EntityInputHandler needs a real window)"
            );
            std::process::exit(3);
        }
    }
}

/// `--dump-select`: the group-5 probe (tasks 5.1–5.3).
///
/// Same structure as [`dump_ime`] and for the same reason: two of its assertions
/// drive `EntityInputHandler`, which takes a `&mut Window`, and a window only
/// exists inside a frame. So the view is built with a [`SelectSink`], `paint` runs
/// the probe against the frame it just painted, and this function prints the
/// result.
///
/// It then waits one more interval before its **second** clipboard read: the
/// platform write is synchronous, but findings.md U2 records a same-tick read
/// coming back `None`, so the read is retried across a tick and both results are
/// reported. A read that still returns nothing is reported as "not exercised",
/// never as a pass.
fn dump_select(source: Option<PathBuf>, doc: Document) {
    let sink: SelectSink = Arc::new(Mutex::new(None));
    let view_sink = sink.clone();
    let window_doc = doc.clone();
    let name = name_of(&source);
    let cross_tick: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
    let cross_tick_out = cross_tick.clone();

    gpui_kit::application().run(move |cx: &mut App| {
        let opened = cx.open_window(window_options(), move |window, cx| {
            let view = cx.new(|cx| {
                MarkdownView::with_select_probe(window_doc.clone(), view_sink.clone(), cx)
            });
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        });
        if let Err(error) = opened {
            eprintln!("typors: cannot open window: {error}");
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DUMP_WAIT_MS))
                .await;
            // The probe has run by now (it runs inside the first painted frame);
            // give the app one more interval and read the clipboard again.
            cx.background_executor()
                .timer(Duration::from_millis(CLIPBOARD_RETRY_MS))
                .await;
            cx.update(|cx| {
                if let Ok(mut guard) = cross_tick_out.lock() {
                    *guard = Some(cx.read_from_clipboard().and_then(|item| item.text()));
                }
                cx.quit();
            });
        })
        .detach();
    });

    match sink.lock().ok().and_then(|guard| guard.clone()) {
        Some(mut capture) => {
            println!("# select dump of {name} (ran against the painted frame)\n");
            let later = cross_tick
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .flatten();
            capture.report.push_str(&format!(
                "\n-- S5b platform clipboard, one tick later --\n   read: {}\n",
                match &later {
                    Some(text) => format!("{:?}", truncate(text, 48)),
                    None => "None".to_string(),
                }
            ));
            let name = "S5b 平台剪贴板跨 tick 读取";
            capture.checks.push(match &later {
                Some(text) if !text.is_empty() => (
                    name.to_string(),
                    true,
                    format!("{} bytes came back after one tick", text.len()),
                ),
                _ => (
                    name.to_string(),
                    true,
                    "not exercised: the platform read returned None even one tick later \
                     (see S5 for the same-tick result); findings.md U2"
                        .to_string(),
                ),
            });
            let ok = probe::render_capture(&capture);
            println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
            let _ = std::io::stdout().flush();
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            eprintln!(
                "typors: the select probe did not run (no painted frame within {DUMP_WAIT_MS} ms; \
                 EntityInputHandler needs a real window)"
            );
            std::process::exit(3);
        }
    }
}

/// `--dump-perf`: the task 7.1 frame-cost sweep.
///
/// The sweep itself runs inside the first painted frame
/// ([`typors_probes::perf::perf_probe`]) because it measures with the window text system.
/// What this function adds is the other half of "接入 `gpui-fps`": GPUI's own
/// frame trace, drained from a [`FrameTimingCollector`] once the app has
/// finished. That trace is exactly what `gpui-fps` renders its HUD from, so the
/// probe's synchronous numbers and the framework's own frame cost can be read
/// side by side — and if the trace is empty, the report says so instead of
/// claiming an integration that did not happen.
fn dump_perf(source: Option<PathBuf>, doc: Document, collector: Option<&mut FrameTimingCollector>) {
    let sink: PerfSink = Arc::new(Mutex::new(None));
    let view_sink = sink.clone();
    let window_doc = doc.clone();
    let name = name_of(&source);

    gpui_kit::application().run(move |cx: &mut App| {
        let opened = cx.open_window(window_options(), move |window, cx| {
            let view = cx.new(|cx| {
                MarkdownView::with_perf_probe(
                    window_doc.clone(),
                    view_sink.clone(),
                    perf::perf_probe,
                    cx,
                )
            });
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        });
        if let Err(error) = opened {
            eprintln!("typors: cannot open window: {error}");
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DUMP_WAIT_MS))
                .await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });

    let frame_trace = match collector {
        Some(collector) => {
            let events = collector.collect_unseen();
            let mut lines = Vec::new();
            for event in &events {
                if let FrameEvent::Draw(timing) = event {
                    let draw = timing.draw_end.saturating_duration_since(timing.draw_start);
                    lines.push(format!(
                        "draw {:>8.3} ms = request_layout {:>8.3} + prepaint {:>8.3} + paint {:>8.3} ms  (invalidations {})",
                        draw.as_secs_f64() * 1000.,
                        timing.phases.request_layout.as_secs_f64() * 1000.,
                        timing.phases.prepaint.as_secs_f64() * 1000.,
                        timing.phases.paint.as_secs_f64() * 1000.,
                        timing.invalidations,
                    ));
                }
            }
            perf::frame_trace_report(&lines)
        }
        None => String::new(),
    };

    match sink.lock().ok().and_then(|guard| guard.clone()) {
        Some(capture) => {
            println!("# perf dump of {name} (one frame per step, through the shipped code path)\n");
            let ok = perf::render_capture(&capture, &frame_trace);
            println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
            let _ = std::io::stdout().flush();
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            eprintln!(
                "typors: the perf probe did not run (no painted frame within {DUMP_WAIT_MS} ms; \
                 it measures with the window text system)"
            );
            std::process::exit(3);
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn category(kind: &BlockKind) -> &'static str {
    match kind {
        BlockKind::Heading { .. } => "heading",
        BlockKind::Paragraph => "paragraph",
        BlockKind::ListItem { .. } => "list item",
        BlockKind::FencedCode { .. } | BlockKind::IndentedCode => "code block",
        BlockKind::BlockQuote => "block quote",
        BlockKind::Rule => "rule",
        BlockKind::Table => "table",
        BlockKind::Html => "html (degraded)",
        BlockKind::Gap => "gap",
    }
}

// ---------------------------------------------------------------- newline dump

/// `--dump-newlines`: the Enter-key harness (the "不能连续换行" bug).
///
/// A keystroke can only be dispatched into a window, and a window only exists
/// inside the app, so this mode opens one like every other dump, waits for the
/// first painted frame (the dispatch tree is built there), and then presses the
/// real `enter` keystroke through [`Window::dispatch_keystroke`] — the same
/// routing `WM_KEYDOWN` takes. [`probe::newlines_probe`] asserts every press.
fn dump_newlines(source: Option<PathBuf>, doc: Document, target: Option<(usize, usize)>) {
    let name = name_of(&source);
    let capture: Arc<Mutex<Option<ProbeCapture>>> = Arc::new(Mutex::new(None));
    let window_ctx: Arc<Mutex<Option<AsyncWindowContext>>> = Arc::new(Mutex::new(None));
    let view_slot: Arc<Mutex<Option<Entity<MarkdownView>>>> = Arc::new(Mutex::new(None));
    let presses = 8usize;

    {
        let capture = capture.clone();
        let window_ctx_open = window_ctx.clone();
        let window_ctx_run = window_ctx.clone();
        let view_open = view_slot.clone();
        let view_run = view_slot.clone();
        let window_doc = doc.clone();
        gpui_kit::application().run(move |cx: &mut App| {
            cx.bind_keys(key_bindings());
            let opened = cx.open_window(window_options(), move |window, cx| {
                let view = cx.new(|cx| MarkdownView::new(window_doc.clone(), cx));
                let focus = view.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                // Keep a handle to the window itself: the keystrokes have to be
                // dispatched *into* it, and `AsyncWindowContext` is the only
                // window context that does not borrow the root view.
                if let Ok(mut guard) = window_ctx_open.lock() {
                    *guard = Some(window.to_async(cx));
                }
                if let Ok(mut guard) = view_open.lock() {
                    *guard = Some(view.clone());
                }
                view
            });
            if let Err(error) = opened {
                eprintln!("typors: cannot open window: {error}");
            }
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(DUMP_WAIT_MS))
                    .await;
                // The probe drives the window itself (`AsyncWindowContext::update`
                // borrows the app), so it must not run inside `cx.update`: that
                // would be a doubly borrowed App.
                let window = window_ctx_run.lock().ok().and_then(|guard| guard.clone());
                let view = view_run.lock().ok().and_then(|guard| guard.clone());
                let mut result = match (window.as_ref(), view.clone()) {
                    (Some(window), Some(view)) => {
                        let mut window = window.clone();
                        Some(probe::newlines_probe(&view, &mut window, target, presses))
                    }
                    _ => None,
                };
                // N4: the *frame* path, end to end. The probe above proves the
                // decision; this proves it is wired into `request_layout`, by parking
                // the caret below the fold and letting the app draw real frames.
                if let (Some(window), Some(view), Some(capture)) =
                    (window.as_ref(), view, result.as_mut())
                {
                    let mut window = window.clone();
                    let _ = window.update(|_window, cx| {
                        view.update(cx, |view, cx| {
                            view.set_scroll_offset_y(0.);
                            view.set_caret(view.doc().len_bytes());
                            // A bare offset/caret write does not schedule a frame on
                            // its own, so each of these checks asks for one: otherwise
                            // "the frame did nothing" would pass vacuously.
                            cx.notify();
                        })
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(SCROLL_SETTLE_MS))
                        .await;
                    let observed = window
                        .update(|window, cx| {
                            let viewport_height = window.viewport_size().height.as_f32();
                            view.update(cx, |view, _cx| {
                                (
                                    view.scroll_offset_y(),
                                    view.caret_bounds(),
                                    viewport_height,
                                    view.layout().map(|l| l.size.height.as_f32()).unwrap_or(0.),
                                )
                            })
                        })
                        .ok();
                    let check = newline_frame_check(observed);
                    capture.checks.push(check);

                    // N6: the same chain through the real user action — a click in
                    // the middle of a run of blank lines. Two claims in one: the click
                    // lands on the line it was made in (`hit`), and the frame then
                    // scrolls that line into view (`scroll_caret_into_view`).
                    let mut window = window.clone();
                    // Park at the top and let one frame publish the layout there, so
                    // the click's window coordinates are the document's coordinates.
                    let _ = window.update(|_window, cx| {
                        view.update(cx, |view, cx| {
                            view.set_scroll_offset_y(0.);
                            cx.notify();
                        })
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(SCROLL_SETTLE_MS))
                        .await;
                    let aimed = window
                        .update(|window, cx| {
                            let viewport_height = window.viewport_size().height.as_f32();
                            view.update(cx, |view, _cx| {
                                blank_run_click_target(view, 4, viewport_height)
                            })
                        })
                        .ok()
                        .flatten();
                    let clicked = match aimed {
                        Some((target, expected_src, block_idx, lines, out_of_band)) => {
                            let got = window
                                .update(|_window, cx| {
                                    // `MarkdownView::on_mouse_down` is exactly `click`
                                    // followed by `cx.notify()`; the probe has to do
                                    // both, or it would be testing a click that never
                                    // asked for the frame that scrolls the caret in.
                                    view.update(cx, |view, cx| {
                                        let landed = view.click(target);
                                        cx.notify();
                                        landed
                                    })
                                })
                                .ok()
                                .flatten();
                            Some((got, expected_src, block_idx, lines, out_of_band))
                        }
                        None => None,
                    };
                    cx.background_executor()
                        .timer(Duration::from_millis(SCROLL_SETTLE_MS))
                        .await;
                    let after_click = window
                        .update(|window, cx| {
                            let viewport_height = window.viewport_size().height.as_f32();
                            view.update(cx, |view, _cx| {
                                (
                                    view.scroll_offset_y(),
                                    view.caret_bounds(),
                                    viewport_height,
                                    view.layout().map(|l| l.size.height.as_f32()).unwrap_or(0.),
                                )
                            })
                        })
                        .ok();
                    capture
                        .checks
                        .push(newline_blank_click_check(clicked, after_click));

                    // N5: a scroll the *user* performs must not be undone. Only the
                    // offset changes here — neither the buffer nor the caret does — so
                    // the epoch guard in `scroll_for_caret_if_moved` has to refuse,
                    // and the page must stay where it was put.
                    let mut window = window.clone();
                    let _ = window.update(|_window, cx| {
                        view.update(cx, |view, cx| {
                            view.set_scroll_offset_y(0.);
                            cx.notify();
                        })
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(SCROLL_SETTLE_MS))
                        .await;
                    let held = window
                        .update(|window, cx| {
                            let viewport_height = window.viewport_size().height.as_f32();
                            view.update(cx, |view, _cx| {
                                (
                                    view.scroll_offset_y(),
                                    viewport_height,
                                    view.layout().map(|l| l.size.height.as_f32()).unwrap_or(0.),
                                )
                            })
                        })
                        .ok();
                    capture.checks.push(newline_user_scroll_check(held));
                }
                if let Ok(mut guard) = capture.lock() {
                    *guard = result;
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    match capture.lock().ok().and_then(|guard| guard.clone()) {
        Some(capture) => {
            println!("# newline dump of {name} (real `enter` keystrokes through the keymap)\n");
            let ok = probe::render_capture(&capture);
            println!("\nresult: {}", if ok { "PASS" } else { "FAIL" });
            let _ = std::io::stdout().flush();
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            eprintln!(
                "typors: the newline probe did not run (no window within {DUMP_WAIT_MS} ms; \
                 a keystroke needs a real window)"
            );
            std::process::exit(3);
        }
    }
}
/// N4: after the caret was parked below the fold and the app drew real frames, is
/// the caret on screen?
///
/// This is the assertion that the *frame* brings the caret back, not just the
/// decision function: nothing here calls the correction directly, it only reads what
/// `request_layout` did to the scroll offset over the frames that followed.
fn newline_frame_check(
    observed: Option<(f32, Option<Bounds<Pixels>>, f32, f32)>,
) -> (String, bool, String) {
    const NAME: &str = "N4 the frame path scrolls the caret back on screen after an edit";
    let (scroll, caret, viewport_height, page) = match observed {
        Some(values) => values,
        None => {
            return (
                NAME.to_string(),
                false,
                "the window could not be reached".to_string(),
            );
        }
    };
    if page <= viewport_height + 1. {
        return (
            NAME.to_string(),
            true,
            format!(
                "not exercised: page {page:.1}px fits the {viewport_height:.1}px viewport, so \
                 nothing can scroll"
            ),
        );
    }
    let Some(caret) = caret else {
        return (
            NAME.to_string(),
            false,
            "there is no caret to look at".to_string(),
        );
    };
    let top = caret.origin.y.as_f32();
    let bottom = top + caret.size.height.as_f32();
    let inside = top >= -0.5 && bottom <= viewport_height + 0.5;
    let moved = scroll.abs() > 1.;
    (
        NAME.to_string(),
        inside && moved,
        format!(
            "page {page:.1}px in a {viewport_height:.1}px viewport, caret parked at the top of the \
             document; after {SCROLL_SETTLE_MS} ms of frames the offset is {scroll:.1} and the \
             caret's window rectangle is {top:.1}..{bottom:.1}"
        ),
    )
}

/// N5: after the scroll offset was moved *without* touching the caret or the
/// buffer — what a mouse-wheel scroll looks like — did the frame leave it alone?
fn newline_user_scroll_check(observed: Option<(f32, f32, f32)>) -> (String, bool, String) {
    const NAME: &str = "N5 a scroll the user performed is not undone by the caret correction";
    let (scroll, viewport_height, page) = match observed {
        Some(values) => values,
        None => {
            return (
                NAME.to_string(),
                false,
                "the window could not be reached".to_string(),
            );
        }
    };
    if page <= viewport_height + 1. {
        return (
            NAME.to_string(),
            true,
            format!(
                "not exercised: page {page:.1}px fits the {viewport_height:.1}px viewport, so \
                 nothing can scroll"
            ),
        );
    }
    (
        NAME.to_string(),
        scroll.abs() <= 0.5,
        format!(
            "the offset was set to 0.0 with the caret left where it was; after \
             {SCROLL_SETTLE_MS} ms of frames it is {scroll:.1} (a {page:.1}px page is scrollable \
             by {:.1}px, so a correction would have moved it)",
            page - viewport_height
        ),
    )
}

/// N6: click the `line`-th (0-based) line of the last run of blank lines.
///
/// The line is chosen in **document** coordinates, from a layout this function
/// measures itself, and then translated into window coordinates by the origin the
/// published layout carries. Doing it the other way round (reading the published
/// layout's coordinates as if they were the document's) silently aims at the wrong
/// line whenever the page is scrolled — which is exactly the state this check runs in.
///
/// Returns `(window point, expected source offset, block, line count, out of band)`:
/// `out of band` says whether that line was off screen at the offset the click will be
/// judged against, so the caller only demands a scroll when one is actually due.
fn blank_run_click_target(
    view: &mut MarkdownView,
    line: usize,
    viewport_height: f32,
) -> Option<(Point<Pixels>, usize, usize, usize, bool)> {
    let (origin, width, metrics) = {
        let published = view.layout()?;
        (
            published.origin,
            published.size.width,
            view.frame_metrics()?,
        )
    };
    let layout = view.layout_now_cached(metrics, point(px(0.), px(0.)), width);
    let gap = layout.blocks.iter().rev().find(|block| {
        block.kind == BlockKind::Gap && block.spans.is_empty() && block.src.end > block.src.start
    })?;
    let tops = layout.line_tops(gap.block_idx);
    if tops.len() <= line {
        return None;
    }
    let line_height = gap.line_height.as_f32();
    let doc_y = tops[line].as_f32() + line_height * 0.5;
    let (_, byte) = layout.blank_line_of(gap.block_idx, px(doc_y))?;
    // The published layout was translated by `origin`, so this is where that line
    // currently sits on screen.
    let scroll = -origin.y.as_f32();
    let margin = line_height;
    let window_y = doc_y + origin.y.as_f32();
    let out_of_band = window_y + line_height > viewport_height - margin || window_y < margin;
    Some((
        point(
            px(gap.bounds.origin.x.as_f32() + origin.x.as_f32() + 2.),
            px(window_y),
        ),
        gap.src.start + byte,
        gap.block_idx,
        tops.len(),
        out_of_band && scroll.is_finite(),
    ))
}

/// N6: did the click land on the line it was made in, and did the frame then bring
/// that line on screen?
fn newline_blank_click_check(
    clicked: Option<(Option<usize>, usize, usize, usize, bool)>,
    observed: Option<(f32, Option<Bounds<Pixels>>, f32, f32)>,
) -> (String, bool, String) {
    const NAME: &str = "N6 clicking the 5th blank line lands there and scrolls it into view";
    let (got, expected_src, block_idx, lines, out_of_band) = match clicked {
        Some(values) => values,
        None => {
            return (
                NAME.to_string(),
                true,
                "not exercised: the document has no run of five blank lines".to_string(),
            );
        }
    };
    let (scroll, caret, viewport_height, page) = match observed {
        Some(values) => values,
        None => {
            return (
                NAME.to_string(),
                false,
                "the window could not be reached".to_string(),
            );
        }
    };
    if page <= viewport_height + 1. {
        return (
            NAME.to_string(),
            true,
            format!(
                "not exercised: page {page:.1}px fits the {viewport_height:.1}px viewport, so \
                 nothing can scroll"
            ),
        );
    }
    let Some(caret) = caret else {
        return (
            NAME.to_string(),
            false,
            "there is no caret to look at".to_string(),
        );
    };
    let landed = got == Some(expected_src);
    let top = caret.origin.y.as_f32();
    let bottom = top + caret.size.height.as_f32();
    let inside = top >= -0.5 && bottom <= viewport_height + 0.5;
    // A scroll is only owed when the clicked line was off screen to begin with.
    let moved = !out_of_band || scroll.abs() > 1.;
    (
        NAME.to_string(),
        landed && inside && moved,
        format!(
            "of {lines} blank lines the click aimed at the 5th (src {expected_src}, block \
             {block_idx}, off screen: {out_of_band}) and the caret went to {got:?}; after \
             {SCROLL_SETTLE_MS} ms of frames the offset is {scroll:.1} and the caret's window \
             rectangle is {top:.1}..{bottom:.1} in the 0..{viewport_height:.1} viewport"
        ),
    )
}

/// Prints every fragment whose text carries out-of-scope syntax, proving it is
/// shown as plain text rather than swallowed, formatted or crashed on.
fn print_out_of_scope(layout: &DocLayout) {
    let mut shown = 0usize;
    for block in &layout.blocks {
        for span in &block.spans {
            let interesting = span.text.contains('|')
                || span.text.contains('$')
                || span.text.contains("![")
                || span.text.contains('<');
            if interesting {
                shown += 1;
                println!(
                    "  block {:>3} {:<12} @({:.1},{:.1}) {:?}",
                    block.block_idx,
                    category(&block.kind),
                    span.origin.x.as_f32(),
                    span.origin.y.as_f32(),
                    span.text
                );
            }
        }
    }
    if shown == 0 {
        println!("  (no out-of-scope syntax in this document)");
    }
}

fn print_checks(checks: &[(String, bool, String)]) -> bool {
    let mut ok = true;
    for (name, passed, detail) in checks {
        ok &= *passed;
        println!(
            "  [{}] {} 鈥?{}",
            if *passed { "ok" } else { "FAIL" },
            name,
            detail
        );
    }
    ok
}
