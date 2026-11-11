# Design

## Context

`mode_for` currently returns `LayoutMode::source(caret_block)` for every caret block. That preserves byte mapping trivially but is exactly the behavior Typora does not have. The renderer already has a rendered projection and `display_byte_for_src`; the missing piece is a caret-local marker visibility layer.

## Decisions

1. Replace `LayoutMode::source(block)` for headings/lists/quotes/code with `Rendered` and add `hidden_markers` metadata to `BlockBox`. The block's `render` and spans stay authoritative; markers remain in the source map but paint with zero width.
2. `caret_display_position` maps the source caret through `src_to_render_offset`; caret x/y uses the rendered line geometry. Clicking the left edge maps to the first visible glyph, not a marker.
3. Fenced code: once `src` contains a complete opening fence + newline, `block_geometry` paints the code plate and content and hides the two fence lines; the caret maps into the content.
4. Chart/mermaid: `mode_for` keeps `source(block)` while the caret is inside a block whose spans contain `ObjectKind::Chart/Diagram`; otherwise rendered. This is the only remaining source block.
5. Inline markers: marker spans adjacent within one char of the caret are painted at normal width; others zero-width. This is a paint-time decision and does not change mapping.
6. Probes: update A5 to assert "caret block rendered, block markers hidden"; C6 to assert the same; S1 selection rects use visible geometry. Add `--dump-live-preview` with heading/list/code/chart transitions.

## Risks

- Hidden markers make caret x and click mapping ambiguous at the zero-width run; add tests at every marker boundary.
- Code fence hiding changes block height and scroll anchoring; reuse the existing scroll anchor.
- Existing source-projection tests encode the old contract; update them intentionally, not by weakening assertions.
