# markdown-completion Specification Delta

## Purpose

Gives Markdown authoring the small, high-value set of syntax suggestions confirmed by the user, styled and behaving like a Typora suggest popup.

## ADDED Requirements

### Requirement: Triggered snippet catalog

Typing one of these prefixes at the caret SHALL offer the matching snippet: `#`/`##`/`###` headings, `-`/`1.` lists, `>` quote, ```` ``` ```` fenced code, `**` bold, `*` italic, `~~` strikethrough, backtick inline code, `$` inline math, `$$` display math, `[` link, `![` image, `|` table and `---` rule. The catalog SHALL contain no language-server, word or path completion.

#### Scenario: Heading suggestion

- **WHEN** the user types `##` at the start of a line
- **THEN** a popup offers `## 标题` and selecting it inserts the heading prefix with the caret after it

#### Scenario: Code fence suggestion

- **WHEN** the user types three backticks
- **THEN** the popup offers a fenced-code template and inserting it places the caret inside the fence

### Requirement: Context and ranking

Suggestions SHALL be filtered by the typed prefix and current block context (for example no inline-math suggestion inside a fenced code block, no heading suggestion mid-paragraph). The closest prefix match SHALL rank first. When nothing matches the popup SHALL not appear.

#### Scenario: No popup without a match

- **WHEN** the user types ordinary prose
- **THEN** no completion popup is shown

#### Scenario: Prefix ranking

- **WHEN** the user types `**` and both bold and a longer emphasis snippet match
- **THEN** bold is the first highlighted item

### Requirement: Popup interaction

The popup SHALL appear next to the caret, above or below whichever side has room, with Typora's popup colors, 1 px border and shadow. `↑/↓` move, `Enter`/`Tab` accept, `Esc` dismisses, and clicking an item accepts it. Accepting replaces the typed prefix with the full snippet in one undoable edit and leaves the caret at the snippet's designated stop.

#### Scenario: Accept with Enter

- **WHEN** the popup is open and the first item is highlighted
- **THEN** Enter inserts that snippet, closes the popup, and one Ctrl+Z removes the whole insertion

#### Scenario: Escape dismisses

- **WHEN** the popup is open and the user presses Esc
- **THEN** the popup closes, the typed prefix stays in the buffer and focus remains in the editor

### Requirement: IME and settings

The popup SHALL NOT open while an IME composition is active, and committing or cancelling a composition SHALL not leave a stale popup. Settings SHALL provide an enable switch and a debounce delay; disabling completion closes any open popup immediately.

#### Scenario: Disabled completion

- **WHEN** completion is turned off in settings and the user types `##`
- **THEN** no popup appears and the text is inserted normally

#### Scenario: Composition

- **WHEN** the user types `#` as part of an active IME composition
- **THEN** no popup appears until the composition commits
