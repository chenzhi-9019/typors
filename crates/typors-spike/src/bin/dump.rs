//! Inspection tool for the H3 mapping: dumps the block/span table of a file.
//!
//! `cargo run --release --bin dump -- sample.md`

use std::env;

use typors_spike::{Document, SpanKind};

fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: dump <file.md>");
        std::process::exit(2);
    });
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        eprintln!("dump: cannot read {path}: {error}");
        std::process::exit(2);
    });

    let doc = Document::parse(&text);
    println!(
        "{} bytes, {} blocks, revision {}",
        doc.len_bytes(),
        doc.blocks().len(),
        doc.revision()
    );
    for (i, block) in doc.blocks().iter().enumerate() {
        println!(
            "block {i}: {:?} src={:?} render={:?}",
            block.kind, block.src, block.render
        );
        for (j, span) in block.spans.iter().enumerate() {
            let kind = match span.kind {
                SpanKind::Text => "text".to_string(),
                SpanKind::Degraded => "degraded".to_string(),
                SpanKind::Marker(m) => format!("marker({m:?})"),
            };
            println!(
                "  span {j}: {kind:<24} src={:?} render={:?} style={:?} src_text={:?}",
                span.src,
                span.render,
                span.style,
                &text[span.src.clone()]
            );
        }
    }

    println!("-- offset map (first 40 source offsets) --");
    for src in (0..=doc.len_bytes()).take(40) {
        let pos = doc.src_to_render(src);
        let back = doc.render_to_src(&pos);
        let visible = doc.src_to_render_offset(src);
        println!("  src {src:<4} -> {pos:?} -> {back:?} visible {visible:?}");
    }
}
