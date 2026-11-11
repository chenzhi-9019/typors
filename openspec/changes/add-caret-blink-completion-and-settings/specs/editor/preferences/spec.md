# preferences Specification Delta

## Purpose

Adds a persistent, Typora-styled preferences surface that controls appearance, file behavior, editing behavior, shortcuts and language.

## ADDED Requirements

### Requirement: Settings surface

Preferences SHALL open as a modal surface inside the main window with a 240 px left category rail and a scrolling right pane, matching Typora's preferences layout and colors: `#fafafa` rail, `#ffffff` pane, `#e9e9e9` separators, 13 px controls, 12 px hints. The rail SHALL list 外观, 文件与保存, 编辑, 快捷键与语言. Opening settings SHALL NOT edit the document or move the caret; `Esc`/取消 closes without applying pending edits and 确定 applies them.

#### Scenario: Open and close

- **WHEN** the user opens Preferences and presses Esc
- **THEN** the surface closes, no setting changes, and focus returns to the editor caret

#### Scenario: Category switching

- **WHEN** the user selects 编辑
- **THEN** the right pane shows only completion, caret and pairing controls and the rail highlight moves

### Requirement: Appearance settings

The 外观 pane SHALL provide theme (浅色 / 深色 / 跟随系统), font family, font size (12–24), line height (1.2–2.0) and page width (600–1200). Changes SHALL apply to the live page immediately and persist. Defaults SHALL be Typora light: `Open Sans`, 16 px, 1.6, 860 px.

#### Scenario: Font size live preview

- **WHEN** the user changes font size from 16 to 18
- **THEN** the page text and caret line height update immediately and remain 18 after restart

#### Scenario: Invalid value

- **WHEN** the user enters a font size outside 12–24
- **THEN** the field is marked invalid and 确定 is disabled until corrected

### Requirement: File and save settings

The 文件与保存 pane SHALL provide autosave (off / 30 s / 60 s / on focus loss), prompt on close, restore last session, and default save directory. Autosave SHALL only write documents that already have a path; untitled documents still use Save As.

#### Scenario: Autosave writes a path

- **WHEN** autosave is 30 s and the document has a path
- **THEN** an edit is written within the interval and the dirty marker clears

#### Scenario: Restore last session

- **WHEN** restore is enabled and typors restarts
- **THEN** the last file reopens at the saved caret/scroll position, or an empty document if the file is gone

### Requirement: Editing settings

The 编辑 pane SHALL provide completion enabled and delay (0–500 ms), caret blinking enabled and interval (300–1200 ms), and bracket/quote pairing. Values SHALL be validated and applied without restart.

#### Scenario: Disable completion

- **WHEN** completion is switched off
- **THEN** any open popup closes and typing Markdown prefixes inserts plain text

#### Scenario: Slow blink

- **WHEN** the caret interval is set to 900 ms
- **THEN** the caret uses 900 ms visible/hidden phases immediately

### Requirement: Shortcuts and language

The 快捷键与语言 pane SHALL show the current menu shortcut table and allow rebinding a menu command by capturing the next key chord; conflicts SHALL be rejected with the conflicting command named. It SHALL offer 简体中文 / English / 跟随系统, applied to menu, settings and dialogs without restart.

#### Scenario: Rebind a shortcut

- **WHEN** the user rebinds 保存 from Ctrl+S to Ctrl+Shift+S
- **THEN** the new chord saves and the old chord no longer does

#### Scenario: Conflict is rejected

- **WHEN** the user assigns a chord already used by 打开
- **THEN** the change is refused and the dialog names 打开

#### Scenario: Language switch

- **WHEN** the user selects English and confirms
- **THEN** the menu and settings labels switch to English and the choice persists

### Requirement: Persistence and reset

Settings SHALL persist as versioned JSON under `%APPDATA%\typors\settings.json`, written atomically. A missing/corrupt file SHALL fall back to defaults and report the recovery without blocking startup. 恢复默认值 SHALL restore every category after confirmation.

#### Scenario: Corrupt settings

- **WHEN** the JSON cannot be parsed at startup
- **THEN** typors starts with defaults, preserves the corrupt file as `.bak`, and shows a non-blocking warning

#### Scenario: Reset

- **WHEN** the user chooses 恢复默认值 and confirms
- **THEN** every setting returns to its documented default and the UI reflects it
