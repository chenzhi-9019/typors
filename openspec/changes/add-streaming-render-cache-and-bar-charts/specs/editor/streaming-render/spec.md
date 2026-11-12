# streaming-render Specification Delta

## Purpose

Defines how typors turns a document into painted frames without doing whole-document work first: the viewport is laid out and painted before anything else, the rest of the document is measured in the background, and edits only reparse what they touch.

## ADDED Requirements

### Requirement: Viewport-first first paint

On opening a document of any size, the first painted frame SHALL show correctly laid-out content for the blocks intersecting the visible viewport plus the configured margin, within the frame budget. It SHALL NOT wait for a whole-document parse or a whole-document layout before painting.

#### Scenario: Large file opens on its first page

- **WHEN** a 10 MB Markdown file is opened
- **THEN** the first frame paints the first page with text, headings and decorations laid out, and the window is interactive before the whole document has been measured

#### Scenario: Visible band is never blank

- **WHEN** a document is opened, scrolled, or resized
- **THEN** no painted frame shows an empty band where visible blocks should be: a block that becomes visible is measured before it is painted

### Requirement: Background measurement of off-screen blocks

Blocks outside the viewport and margin SHALL be measured in the background, nearest to the viewport first, and merged into the layout as results arrive. Background work SHALL NOT block input, caret movement, scrolling or repaint.

#### Scenario: Scrolling into unmeasured content

- **WHEN** the user scrolls quickly to a region that has not been measured yet
- **THEN** that region is measured with priority before it is painted, and the scroll does not stall for the rest of the document

#### Scenario: Typing while the background pass runs

- **WHEN** text is typed while background measurement is in progress
- **THEN** keystrokes are processed without waiting for the background pass, and the visible band reflects the edit immediately

### Requirement: Stable scroll extent

While blocks are still unmeasured, the document SHALL keep a usable scroll extent derived from estimated heights. When a background measurement replaces an estimate, content above the viewport SHALL NOT shift the block under the viewer, and the scrollbar SHALL converge on the true extent without oscillating.

#### Scenario: Scrollbar settles

- **WHEN** a long document finishes background measurement
- **THEN** the scrollbar reaches a stable extent and the caret or scroll anchor stays on the same content

#### Scenario: Measurement above the anchor

- **WHEN** a block above the current scroll position turns out taller than estimated
- **THEN** the viewport is compensated so the content under the viewer does not jump

### Requirement: Chunked document open

Opening a file SHALL read and parse it in sequential chunks so the first frame can paint before the whole file is parsed. Chunk boundaries SHALL NOT split a block in the final document model, and the completed document SHALL be identical to a whole-file parse of the same bytes.

#### Scenario: Equivalent document model

- **WHEN** a file is opened through the chunked path and the same file through the whole-file path
- **THEN** both produce the same blocks, source ranges and rendered projection

#### Scenario: Chunk boundary inside a fenced block

- **WHEN** a chunk boundary falls inside a fenced code, chart or Mermaid block
- **THEN** the block is assembled across chunks and renders exactly once

### Requirement: Edits stay local

An edit SHALL reparse only the blocks it can affect, not the whole document document model, and SHALL NOT convert the entire document to a string on the edit path. Documents below the small-document threshold MAY take the whole-document path.

#### Scenario: Typing latency does not grow with file size

- **WHEN** a character is typed in the middle of a large document
- **THEN** the edit, reparse and visible-band relayout complete within the frame budget, and no whole-document string copy is performed

#### Scenario: Edit that changes block structure

- **WHEN** an edit adds or removes a fence, list marker, table row or blank line
- **THEN** the neighbouring blocks whose structure can change are reparsed too, and the resulting blocks match a whole-document parse

### Requirement: Documented frame budget and degradation

The rendering pipeline SHALL have a documented per-frame work budget and a fixed degradation order: visible band first, margin second, background chunks last. When the budget is exhausted, remaining background work SHALL be deferred to a later frame rather than dropped or performed synchronously.

#### Scenario: Budget exhaustion defers background work

- **WHEN** a frame's layout and shaping reach the budget while background chunks remain
- **THEN** the frame paints what it has, defers the remaining chunks, and does not block on them

#### Scenario: Visible work is never deferred

- **WHEN** the budget is already exhausted by visible-band work
- **THEN** visible-band layout still completes, and only background chunks are deferred

### Requirement: Streaming behaviour is observable

The diagnostic modes SHALL report streaming state: first-painted-block latency, blocks measured versus estimated, pending background chunks, and bytes still unparsed. Existing layout, caret, selection, media and performance probes SHALL continue to pass with streaming enabled, and each SHALL be able to run with streaming forced complete.

#### Scenario: Performance probe reports streaming numbers

- **WHEN** `--dump-perf` runs on a large file
- **THEN** it reports first-frame visible-block latency, background completion time, and the measured-versus-estimated block counts, and exits 0

#### Scenario: Probe with streaming forced off

- **WHEN** a probe runs with streaming disabled
- **THEN** it produces the same geometry and caret results as the streaming path once everything is measured
