# Tasks

## 1. Projection

- [x] 1.1 Add hidden-marker metadata to `BlockBox` and make heading/list/quote marker spans zero-width while keeping block style; verify a heading line with caret at end paints no `#` and full-height H2.
- [x] 1.2 Change `mode_for` to rendered for headings/lists/quotes/paragraphs and keep source only for chart/mermaid object blocks; add a test for both.
- [x] 1.3 Make complete fenced code blocks paint the plate/content with fences hidden and the caret inside; test Enter after ```` ```rust ````.
- [ ] 1.4 Add caret-local inline marker visibility; test `**bold**` with caret inside/outside.

## 2. Mapping

- [x] 2.1 Verify `caret_display_position`, click, Home/End and arrow keys with hidden markers at every boundary; fix any off-by-marker offsets.
- [x] 2.2 Update A5/C6/S1 probes for the rendered-caret contract and add `--dump-live-preview` covering heading, list, code, chart and blur.

## 3. Docs and verification

- [x] 3.1 Update README and hybrid-editor docs with the Typora live-preview rules.
- [x] 3.2 Run all probes and `cargo test --workspace`; attach screenshots of heading, code block, chart blur.
