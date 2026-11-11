# unsaved-changes Specification Delta

## Purpose

Prevents silent data loss by routing every close path through a Typora-style save confirmation.

## ADDED Requirements

### Requirement: Dirty tracking

A document SHALL be dirty when its current revision differs from the revision last successfully saved or loaded. New untitled documents SHALL be clean until the first edit. The window title SHALL append `*` while dirty and remove it after a successful save.

#### Scenario: Title marker

- **WHEN** a document is edited
- **THEN** the title shows `*` and it disappears after Save succeeds

### Requirement: Close paths are guarded

File → Exit, the window close button, Alt+F4 and OS session shutdown SHALL all ask before discarding a dirty document. A clean document or one with autosave already written SHALL close without a prompt.

#### Scenario: Close button with edits

- **WHEN** the user clicks the window X with unsaved edits
- **THEN** closing is cancelled and the Save / Don't Save / Cancel dialog is shown

#### Scenario: Clean document closes

- **WHEN** the document has no unsaved changes and the user closes the window
- **THEN** it closes immediately with no dialog

### Requirement: Three-way dialog

The prompt SHALL offer Save, Don't Save and Cancel. Save writes the document and closes only after a successful write; Don't Save closes and discards; Cancel keeps the window open with buffer, caret and selection untouched. A failed save SHALL keep the window open and report the error.

#### Scenario: Save then close

- **WHEN** the user chooses Save and the write succeeds
- **THEN** the file is written and the window closes

#### Scenario: Save fails

- **WHEN** the user chooses Save and the write fails
- **THEN** the window stays open, the buffer is unchanged, and the error is shown

#### Scenario: Cancel

- **WHEN** the user chooses Cancel
- **THEN** the window stays open and the caret is where it was

### Requirement: Autosave interaction

When autosave is enabled and the document has a path, a successful autosave SHALL clear dirty state and the close prompt. Untitled documents SHALL still prompt because there is no path to autosave to.

#### Scenario: Autosaved close

- **WHEN** autosave has written the latest revision and the user closes the window
- **THEN** no prompt appears

#### Scenario: Untitled autosave

- **WHEN** autosave is on but the document has never been saved
- **THEN** closing still asks whether to save
