# caret-and-blank-document Specification Delta

## Purpose

Makes an empty typors window a real editable document and makes the caret's position, size and visibility trustworthy on every line and at every block boundary.

## ADDED Requirements

### Requirement: A blank document is editable

When typors starts without a file, the document SHALL contain at least one editable text block with source length zero. The caret SHALL be available at source offset 0, and a left click anywhere in the empty page SHALL resolve to that block and offset 0.

#### Scenario: Launch without a file

- **WHEN** the user runs `typors` with no path
- **THEN** the page is empty, the caret is at offset 0, and the window title indicates an unnamed document

#### Scenario: Click the empty page

- **WHEN** the user clicks in the blank area below the initial caret
- **THEN** the caret moves to offset 0 of the empty document and remains visible

### Requirement: Caret geometry follows the line box

The caret SHALL be drawn at the text insertion point of the line that owns the source offset, with x derived from that line's shaped text and y/height from that line's line box. This MUST hold at line starts, line ends, soft-wrapped lines, trailing empty lines, consecutive blank lines, and around atomic objects.

#### Scenario: Caret at the start of a new line

- **WHEN** the user presses Enter at the end of a paragraph
- **THEN** the caret's y moves to the new line's top and its x is the content left edge, not the previous line's end

#### Scenario: Caret in a run of blank lines

- **WHEN** the document contains several consecutive empty lines and the user clicks the third one
- **THEN** the caret is drawn on the third line and typing inserts at that line's source offset

#### Scenario: Caret after an atomic object

- **WHEN** the caret is immediately after an inline image or formula
- **THEN** it is drawn at the object's right edge on the object's line, with the line's height

#### Scenario: Caret at the end of the document

- **WHEN** the document ends with a newline and the caret is at the end offset
- **THEN** the caret is drawn on the final empty line, not on the preceding line

### Requirement: Caret size is stable and legible

The caret SHALL be 2 logical pixels wide and exactly the owning line's height, independent of the block's font size, zoom level, DPI scale, or whether the line contains CJK, emoji, inline code, or a heading. Its color MUST meet the active theme's caret contrast.

#### Scenario: Caret in a heading

- **WHEN** the caret sits in an `H1` line
- **THEN** the caret height matches the heading line box and it is not drawn using body-text height

#### Scenario: Caret at high DPI

- **WHEN** the window is on a display with a scale factor other than 1.0
- **THEN** the caret's rendered width remains visually 2 logical pixels and its height matches the line

### Requirement: No stale or disappearing caret

Exactly one caret SHALL be painted while the editor is focused. Moving the caret, scrolling, editing, opening and closing a menu, or switching between rendered and source projection MUST repaint the caret at its new bounds and MUST NOT leave a caret painted at an old position. The caret MAY be hidden while the window is unfocused and MUST reappear at the same document position when focus returns.

#### Scenario: Scroll does not leave a ghost

- **WHEN** the user scrolls the document so the caret moves on screen
- **THEN** paint output contains one caret at the new screen position and none at the old position

#### Scenario: Menu focus round-trip

- **WHEN** the user opens a menu and closes it without running a command
- **THEN** the caret reappears at the same document offset and no duplicate caret is painted

#### Scenario: Window loses focus

- **WHEN** the window becomes inactive and later regains focus
- **THEN** the caret is hidden while inactive and visible at the unchanged offset after focus returns

### Requirement: Typing and IME work from a blank document

With an empty buffer, printable keys, `Enter`, paste and IME composition SHALL insert at the caret without a prior click. The composition text SHALL be visible, the platform candidate window SHALL be anchored at the caret, and committing or cancelling MUST leave the caret at the correct offset.

#### Scenario: Type the first character

- **WHEN** the user launches without a file and types `你`
- **THEN** the buffer contains exactly `你`, the caret is after it, and the character is painted

#### Scenario: Enter on an empty document

- **WHEN** the user presses Enter on the empty document
- **THEN** the buffer contains one newline and the caret is on the second line

#### Scenario: IME composition on an empty document

- **WHEN** the user starts an IME composition as the first input in the document
- **THEN** the composition text is painted at the caret, the candidate window is anchored there, and committing inserts the composed text at offset 0

### Requirement: Replacing the document resets caret state

New, Open and any command that replaces the buffer SHALL clear the previous selection, composition, undo history and caret position, then place the caret at offset 0 of the new document. The old document's caret or selection MUST NOT be painted over the new content.

#### Scenario: New after editing

- **WHEN** the user edits a document, leaves the caret in the middle, and runs `文件 → 新建`
- **THEN** the new empty document shows one caret at offset 0 and no selection from the old document

#### Scenario: Open after editing

- **WHEN** the user opens another Markdown file while a caret and selection exist
- **THEN** the loaded document is shown with the caret at offset 0 and no stale highlight
