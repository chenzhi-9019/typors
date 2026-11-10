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
//! ([`typors_spike::render::DocLayout::checks`]); `--dump-cursor` and `--dump-edit` drive
//! the same [`typors_spike::view::MarkdownView`] the window paints and assert the group-4
//! behaviour ([`typors_spike::probe`]). `--dump-ime` replays the Windows input-method
//! callback order through the same `EntityInputHandler` the platform calls and
//! asserts group 6 ([`typors_spike::ime`]); `--dump-select` does the same for group 5 —
//! selection geometry, the clipboard round trip and the undo history
//! ([`typors_spike::selection`]); `--dump-perf` sweeps the whole document for task 7.1.
//! Exit code 0 means every check passed.

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::gpui::profiler::{FrameEvent, FrameTimingCollector, set_trace_enabled, trace_enabled};
use gpui_kit::gpui::*;
use typors_spike::perf::{self, PerfSink};
use typors_spike::probe::{self, ProbeCapture};
use typors_spike::render::{ApproxMetrics, DocLayout, EditorTheme};
use typors_spike::view::{key_bindings, DumpSink, ImeSink, MarkdownView, SelectSink};
use typors_spike::{BlockKind, Document};

/// How long the dump run waits for the first painted frame before giving up and
/// falling back to approximate metrics.
const DUMP_WAIT_MS: u64 = 900;

/// How long `--dump-select` waits before its *second* clipboard read. findings.md
/// U2 reports that a read in the same tick as the write can come back `None`, so
/// the probe reads once immediately and once after the app has pumped events.
const CLIPBOARD_RETRY_MS: u64 = 120;

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
}

fn main() {
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
                eprintln!("spike: unknown option {other}");
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
    typors_spike::ime::log_ime(
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
    let text = typors_spike::view::normalize_newlines(&text).into_owned();
    let doc = Document::parse(&text);
    eprintln!(
        "spike: loaded {} bytes, {} blocks from {}",
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
    }
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
                    eprintln!("spike: cannot read {}: {error}", path.display());
                    std::process::exit(2);
                }
            }
        }
        None => {
            eprintln!(
                "usage: spike \
                 [--dump-layout|--dump-cursor [<block>:<byte>]|--dump-edit|--dump-newlines \
                 [<block>:<byte>]|--dump-ime|--dump-select|--dump-perf] <file.md>"
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
        let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
            let view = cx.new(|cx| {
                if fps {
                    MarkdownView::with_fps(doc.clone(), cx)
                } else {
                    MarkdownView::new(doc.clone(), cx)
                }
            });
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            if let Ok(mut guard) = view_open.lock() {
                *guard = Some(view.clone());
            }
            view
        });
        match opened {
            Ok(_) => {
                println!("spike: window open on {name} (render projection + caret)");
                println!("spike: click to place the caret, type to edit, arrows to move");
                if fps {
                    println!("spike: gpui-fps HUD on (--fps)");
                }
                // A marker line, so "no `mark` lines at all" is a readable result
                // rather than an empty log (findings.md H2, step V1). It doubles as
                // task 7.2's "input is live" timestamp.
                typors_spike::ime::log_ime(
                    "ready",
                    format_args!(
                        "window open; every IME callback now logs here (TYPORS_IME_LOG=off silences)"
                    ),
                );
                let _ = std::io::stdout().flush();
            }
            Err(error) => {
                eprintln!("spike: cannot open window: {error}");
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
                                typors_spike::view::escape_insert(&text),
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

fn dump_layout(source: Option<PathBuf>, doc: Document) {
    let sink: DumpSink = Arc::new(Mutex::new(None));
    let view_sink = sink.clone();
    let window_doc = doc.clone();
    let name = name_of(&source);

    gpui_kit::application().run(move |cx: &mut App| {
        let opened = cx.open_window(WindowOptions::default(), move |_window, cx| {
            cx.new(|cx| MarkdownView::with_dump(window_doc.clone(), view_sink.clone(), cx))
        });
        if let Err(error) = opened {
            eprintln!("spike: cannot open window ({error}); will dump approximate geometry");
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
            println!("# layout dump of {name} (measured by the window text system, painted frame)\n");
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
                "spike: no painted frame within {DUMP_WAIT_MS} ms; dumping approximate geometry instead"
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
            let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
                let view = cx.new(|cx| MarkdownView::new(window_doc.clone(), cx));
                let focus = view.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                if let Ok(mut guard) = holder_open.lock() {
                    *guard = Some(view.clone());
                }
                view
            });
            if let Err(error) = opened {
                eprintln!("spike: no window ({error}); probing with approximate metrics");
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
            eprintln!("spike: the probe did not run (no frame within {DUMP_WAIT_MS} ms)");
            std::process::exit(3);
        }
    }
}

fn run_kind(kind: &ProbeKind, view: &mut MarkdownView) -> ProbeCapture {
    match kind {
        ProbeKind::Cursor(target) => probe::cursor_probe(view, *target),
        ProbeKind::Edit => probe::edit_probe(view),
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
        let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
            let view = cx.new(|cx| MarkdownView::with_ime_probe(window_doc.clone(), view_sink.clone(), cx));
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        });
        if let Err(error) = opened {
            eprintln!("spike: cannot open window: {error}");
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
                "spike: the ime probe did not run (no painted frame within {DUMP_WAIT_MS} ms; \
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
        let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
            let view =
                cx.new(|cx| MarkdownView::with_select_probe(window_doc.clone(), view_sink.clone(), cx));
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        });
        if let Err(error) = opened {
            eprintln!("spike: cannot open window: {error}");
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
                "spike: the select probe did not run (no painted frame within {DUMP_WAIT_MS} ms; \
                 EntityInputHandler needs a real window)"
            );
            std::process::exit(3);
        }
    }
}

/// `--dump-perf`: the task 7.1 frame-cost sweep.
///
/// The sweep itself runs inside the first painted frame
/// ([`typors_spike::perf::perf_probe`]) because it measures with the window text system.
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
        let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
            let view =
                cx.new(|cx| MarkdownView::with_perf_probe(window_doc.clone(), view_sink.clone(), cx));
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        });
        if let Err(error) = opened {
            eprintln!("spike: cannot open window: {error}");
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
                    let draw = timing
                        .draw_end
                        .saturating_duration_since(timing.draw_start);
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
                "spike: the perf probe did not run (no painted frame within {DUMP_WAIT_MS} ms; \
                 it measures with the window text system)"
            );
            std::process::exit(3);
        }
    }
}

fn truncate(text: &str, max: usize) -> String {    if text.chars().count() <= max {
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
            let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
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
                eprintln!("spike: cannot open window: {error}");
            }
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(DUMP_WAIT_MS))
                    .await;
                // The probe drives the window itself (`AsyncWindowContext::update`
                // borrows the app), so it must not run inside `cx.update`: that
                // would be a doubly borrowed App.
                let mut window = window_ctx_run.lock().ok().and_then(|mut guard| guard.take());
                let view = view_run.lock().ok().and_then(|guard| guard.clone());
                let result = match (window.as_mut(), view) {
                    (Some(window), Some(view)) => {
                        Some(probe::newlines_probe(&view, window, target, presses))
                    }
                    _ => None,
                };
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
                "spike: the newline probe did not run (no window within {DUMP_WAIT_MS} ms; \
                 a keystroke needs a real window)"
            );
            std::process::exit(3);
        }
    }
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
