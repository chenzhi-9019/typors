# editor/typora-live-preview Specification

## Purpose
Defines Typora-style live preview: complete block syntax disappears immediately while its rendered style and the editing caret remain.

## Requirements

### Requirement: Block markers hide after completion

Typing a heading marker plus space, a list marker plus space, or a quote marker plus space SHALL immediately hide the marker and render the line in its block style. Further typing on that line SHALL keep the style and the caret at the typed position.

#### Scenario: Heading marker

- **WHEN** the user types `## ` at the start of a line and then `123`
- **THEN** the line shows `123` in H2 style with no visible `##`, and the caret is after `123`

#### Scenario: List marker

- **WHEN** the user types `- ` and `item`
- **THEN** the line shows the list bullet plus `item`, with no `- ` text

### Requirement: Code fence renders while editing

After the user types an opening fence with an optional language and presses Enter, the fence lines SHALL hide, the code plate SHALL render, and the caret SHALL be inside the code content. Typing code SHALL stay in the rendered block; moving out and back re-enters the same display without requiring a second Enter.

#### Scenario: Fence becomes a code block

- **WHEN** the user types three backticks, `rust`, and Enter
- **THEN** the code block plate is visible and the caret is on the first code line

### Requirement: Chart and mermaid render on blur

A `chart` or `mermaid` fence SHALL stay in source while the caret is inside it. When the caret moves to another block, it SHALL render the chart/diagram; when the caret returns, it SHALL show the fence source again.

#### Scenario: Mermaid blur

- **WHEN** the caret is inside a mermaid fence and the user clicks another block
- **THEN** the mermaid SVG replaces the source

### Requirement: Inline markers are caret-local

Inline emphasis/code markers SHALL be visible only on the fragment adjacent to the caret; otherwise they SHALL be hidden while the styled text remains.

#### Scenario: Bold markers

- **WHEN** the caret is inside `**bold**`
- **THEN** the `**` delimiters are visible at the caret edges and hidden after the caret leaves

### Requirement: Mapping and probes

Source/render mapping SHALL keep caret, selection, Home/End and click positions correct with hidden markers. The A5/C6/S1 probes SHALL be rewritten for the new contract and fail if a hidden marker shifts the caret.

#### Scenario: Click before a hidden heading marker

- **WHEN** the user clicks at the left edge of a rendered heading
- **THEN** the caret lands at the heading text start, not inside a zero-width marker

#### Scenario: Probe contract

- **WHEN** `--dump-layout` runs
- **THEN** A5 asserts the caret block is rendered with hidden block markers, not raw source
