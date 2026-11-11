# app-menu-bar Specification Delta

## Purpose

Gives typors a visible, keyboard-accessible command surface above the editor so every supported action can be discovered and invoked without memorizing shortcuts.

## ADDED Requirements

### Requirement: Menu bar presence and labels

The editor window SHALL show a menu bar above the document area with exactly these top-level entries, in this order: `文件(F)`, `编辑(E)`, `段落(P)`, `格式(O)`, `视图(V)`, `主题(T)`, `帮助(H)`. The bar MUST use the active theme's text, background and border colors and MUST NOT scroll away with the document.

#### Scenario: Menu bar is visible on launch

- **WHEN** the editor window opens for an existing file or for a blank document
- **THEN** the seven top-level entries are visible above the page, and the document content starts below the bar

#### Scenario: Window is narrow

- **WHEN** the window is narrower than the sum of the seven labels
- **THEN** every top-level entry remains reachable (horizontal scrolling or an overflow affordance), and no entry is clipped without a way to open it

### Requirement: Menu open and close interaction

A top-level entry SHALL open its dropdown on left click and on its `Alt+<letter>` mnemonic. Only one dropdown may be open at a time. `Esc`, a click outside the bar and dropdown, or activating a command MUST close it. Moving the pointer or using Left/Right while open MUST switch to the adjacent top-level menu.

#### Scenario: Mouse opens and closes a menu

- **WHEN** the user clicks `文件(F)` and then clicks outside the dropdown
- **THEN** the 文件 dropdown opens and then closes without changing the document

#### Scenario: Keyboard mnemonic opens a menu

- **WHEN** the user presses `Alt+E`
- **THEN** the 编辑 dropdown opens and the editor's text is not modified

#### Scenario: Escape closes the menu

- **WHEN** a dropdown is open and the user presses `Esc`
- **THEN** the dropdown closes and focus returns to the editor at the previous caret

### Requirement: Keyboard navigation of dropdowns

While a dropdown is open, Up/Down SHALL move the highlighted item, skipping disabled items, and Enter/Space SHALL invoke the highlighted item. Left/Right SHALL move to the adjacent top-level menu while keeping a dropdown open. The highlighted item MUST remain visibly distinct.

#### Scenario: Invoke a command with the keyboard

- **WHEN** the user opens `格式(O)`, moves to `粗体` and presses Enter
- **THEN** the same bold action as the toolbar/keyboard shortcut runs exactly once and the dropdown closes

#### Scenario: Disabled items are skipped

- **WHEN** every item in a dropdown is disabled
- **THEN** Up/Down leave the highlight unset and Enter does not invoke anything

### Requirement: Menu commands operate on the current editor state

A menu command SHALL use the same editor entry point as its keyboard shortcut, so caret, selection, undo history, IME composition and rendered projection stay consistent. Opening or closing a menu MUST NOT edit the buffer, move the caret, clear a selection, or end an IME composition.

#### Scenario: Edit command matches its shortcut

- **WHEN** the user selects text and runs `编辑(E) → 复制` from the menu, then pastes it with `Ctrl+V`
- **THEN** the pasted text equals the selection and the document is unchanged by opening the menu

#### Scenario: Undo after a menu command

- **WHEN** the user runs `格式(O) → 粗体` from the menu and then presses `Ctrl+Z`
- **THEN** the buffer returns byte-for-byte to its state before the menu command

### Requirement: File menu commands

The 文件 menu SHALL provide 新建, 打开, 保存, 另存为 and 退出. 新建 MUST replace the current buffer with an empty editable document and clear the current file path. 打开 and 另存为 MUST use a native file dialog filtered to Markdown files. 保存 MUST write the current path, or fall back to 另存为 when the document has no path. A failed read or write MUST leave the current buffer unchanged and report the error without closing the window.

#### Scenario: New blank document

- **WHEN** the user chooses `文件(F) → 新建`
- **THEN** the buffer becomes empty, the title no longer names the previous file, and the caret is ready at position 0

#### Scenario: Save without a path

- **WHEN** a document with no path is saved
- **THEN** a Save As dialog opens, and cancelling it leaves the buffer and path unchanged

#### Scenario: Save failure is non-destructive

- **WHEN** writing the current file fails
- **THEN** the buffer, caret and selection are unchanged and the error is shown to the user

### Requirement: Command availability reflects editor state

Menu items SHALL be disabled when their action cannot apply (for example undo with empty history, copy with no selection, save when the document is read-only). Enabling/disabling MUST update when the caret, selection, history or document path changes, without requiring a restart.

#### Scenario: Copy is disabled without a selection

- **WHEN** the caret is collapsed and the 编辑 menu is opened
- **THEN** 复制 and 剪切 are visibly disabled and do not run if activated

#### Scenario: Undo becomes available after an edit

- **WHEN** the user types a character and then opens the 编辑 menu
- **THEN** 撤销 is enabled and invokes the same undo as `Ctrl+Z`

### Requirement: Menu does not steal typing or focus

After a command runs or the menu closes, keyboard focus SHALL return to the editor with the caret where it was. Clicking a top-level label MUST NOT place a text caret in the menu, and typing while no dropdown is open MUST continue to edit the document.

#### Scenario: Type immediately after closing

- **WHEN** the user closes a dropdown with `Esc` and types `abc`
- **THEN** `abc` is inserted at the editor's caret, not into the menu

#### Scenario: Focus returns after a command

- **WHEN** the user invokes `视图(V) → 放大` from the menu
- **THEN** the menu closes, the editor regains focus, and the caret remains visible at its prior document position
