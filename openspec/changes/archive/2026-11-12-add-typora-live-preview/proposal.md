# Proposal

## Why

The current hybrid editor always paints the caret block as Markdown source. Typing `# ` or `## ` therefore leaves the hashes visible and only styles the text; a fenced code block keeps its backticks while editing; chart/mermaid fences collapse at the wrong time. Typora's live preview hides block markers immediately and keeps the line styled while the caret remains in it.

## What Changes

- Block markers (`# `, `- `, `1. `, `> `) become zero-width as soon as their syntax is complete; the line stays rendered and editable.
- Fenced code: after the info string and Enter, the fence lines hide, the code plate renders, and the caret stays in the code content.
- `chart`/`mermaid` fences remain source while the caret is inside and render only when the block loses focus.
- Inline markers (`**`, `*`, `` ` ``, `~~`) reveal only next to the caret; elsewhere they stay hidden.
- The projection contract changes from "caret block = source" to "caret block = rendered with caret-local markers"; A5/C6/S1 probes and the hybrid spec are updated accordingly.

## Capabilities

### New Capabilities

- `editor/typora-live-preview`: marker hiding rules, code-fence/chart/mermaid transitions, caret mapping and probe contract.
